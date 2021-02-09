//! Callbacks for Git hooks.
//!
//! Git uses "hooks" to run user-defined scripts after certain events. We
//! extensively use these hooks to track user activity and e.g. decide if a
//! commit should be considered "hidden".
//!
//! The hooks are installed by the `branchless init` command. This module
//! contains the implementations for the hooks.

use std::convert::TryInto;
use std::io::{stdin, BufRead, Write};
use std::time::SystemTime;

use anyhow::Context;
use pyo3::prelude::*;

use crate::eventlog::{should_ignore_ref_updates, Event, EventLogDb};
use crate::formatting::Pluralize;
use crate::gc::mark_commit_reachable;
use crate::python::{map_err_to_py_err, TextIO};
use crate::util::{get_db_conn, get_repo};

/// Handle Git's `post-checkout` hook.
///
/// See the man-page for `githooks(5)`.
pub fn hook_post_checkout<Out: Write>(
    out: &mut Out,
    previous_head_ref: &str,
    current_head_ref: &str,
    is_branch_checkout: isize,
) -> anyhow::Result<()> {
    if is_branch_checkout == 0 {
        return Ok(());
    }

    let now = SystemTime::now();
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?;
    writeln!(out, "branchless: processing checkout")?;

    let repo = get_repo()?;
    let conn = get_db_conn(&repo)?;
    let mut event_log_db = EventLogDb::new(conn)?;
    event_log_db
        .add_events(vec![Event::RefUpdateEvent {
            timestamp: timestamp.as_secs_f64(),
            old_ref: Some(String::from(previous_head_ref)),
            new_ref: Some(String::from(current_head_ref)),
            ref_name: String::from("HEAD"),
            message: None,
        }])
        .with_context(|| "Adding events to event-log")?;
    Ok(())
}

/// Handle Git's `post-commit` hook.
///
/// See the man-page for `githooks(5)`.
pub fn hook_post_commit<Out: Write>(out: &mut Out) -> anyhow::Result<()> {
    writeln!(out, "branchless: processing commit")?;

    let repo = get_repo()?;
    let conn = get_db_conn(&repo)?;
    let mut event_log_db = EventLogDb::new(conn)?;

    let commit = repo
        .head()
        .with_context(|| "Getting repo HEAD")?
        .peel_to_commit()
        .with_context(|| "Getting HEAD commit")?;
    mark_commit_reachable(&repo, commit.id())
        .with_context(|| "Marking commit as reachable for GC purposes")?;

    let timestamp = commit.time().seconds() as f64;
    event_log_db
        .add_events(vec![Event::CommitEvent {
            timestamp,
            commit_oid: commit.id(),
        }])
        .with_context(|| "Adding events to event-log")?;

    Ok(())
}

fn parse_reference_transaction_line(now: SystemTime, line: &str) -> anyhow::Result<Option<Event>> {
    match *line.split(' ').collect::<Vec<_>>().as_slice() {
        [old_value, new_value, ref_name] => {
            if !should_ignore_ref_updates(ref_name) {
                let timestamp = now
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .with_context(|| "Processing timestamp")?;
                Ok(Some(Event::RefUpdateEvent {
                    timestamp: timestamp.as_secs_f64(),
                    ref_name: String::from(ref_name),
                    old_ref: Some(String::from(old_value)),
                    new_ref: Some(String::from(new_value)),
                    message: None,
                }))
            } else {
                Ok(None)
            }
        }
        _ => {
            anyhow::bail!(
                "Unexpected number of fields in reference-transaction line: {}",
                &line
            )
        }
    }
}

/// Handle Git's `reference-transaction` hook.
///
/// See the man-page for `githooks(5)`.
pub fn hook_reference_transaction<Out: Write>(
    out: &mut Out,
    transaction_state: &str,
) -> anyhow::Result<()> {
    if transaction_state != "committed" {
        return Ok(());
    }
    let timestamp = SystemTime::now();

    let events: Vec<Event> = stdin()
        .lock()
        .lines()
        .filter_map(|line| {
            let line = match line {
                Ok(line) => line,
                Err(_) => return None,
            };
            match parse_reference_transaction_line(timestamp, &line) {
                Ok(event) => event,
                Err(err) => {
                    log::error!("Could not parse reference-transaction-line: {:?}", err);
                    None
                }
            }
        })
        .collect();
    if events.is_empty() {
        return Ok(());
    }

    let num_reference_updates = Pluralize {
        amount: events.len().try_into()?,
        singular: "update to a branch/ref",
        plural: "updates to branches/refs",
    };
    writeln!(
        out,
        "branchless: processing {}",
        num_reference_updates.to_string()
    )?;

    let repo = get_repo()?;
    let conn = get_db_conn(&repo)?;
    let mut event_log_db = EventLogDb::new(conn)?;
    event_log_db
        .add_events(events)
        .with_context(|| "Adding events to event-log")?;

    Ok(())
}

#[pyfunction]
fn py_hook_post_checkout(
    py: Python,
    out: PyObject,
    previous_head_ref: &str,
    current_head_ref: &str,
    is_branch_checkout: isize,
) -> PyResult<()> {
    let mut out = TextIO::new(py, out);
    let result = hook_post_checkout(
        &mut out,
        previous_head_ref,
        current_head_ref,
        is_branch_checkout,
    );
    map_err_to_py_err(result, "Could not invoke post-checkout hook")?;
    Ok(())
}

#[pyfunction]
fn py_hook_post_commit(py: Python, out: PyObject) -> PyResult<()> {
    let mut out = TextIO::new(py, out);
    let result = hook_post_commit(&mut out);
    map_err_to_py_err(result, "Could not invoke post-commit hook")?;
    Ok(())
}

#[pyfunction]
fn py_hook_reference_transaction(
    py: Python,
    out: PyObject,
    transaction_state: &str,
) -> PyResult<()> {
    let mut out = TextIO::new(py, out);
    let result = hook_reference_transaction(&mut out, transaction_state);
    map_err_to_py_err(result, "Could not invoke reference-transaction hook")?;
    Ok(())
}

#[allow(missing_docs)]
pub fn register_python_symbols(module: &PyModule) -> PyResult<()> {
    module.add_function(pyo3::wrap_pyfunction!(py_hook_post_checkout, module)?)?;
    module.add_function(pyo3::wrap_pyfunction!(py_hook_post_commit, module)?)?;
    module.add_function(pyo3::wrap_pyfunction!(
        py_hook_reference_transaction,
        module
    )?)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_reference_transaction_line() -> anyhow::Result<()> {
        let timestamp = SystemTime::UNIX_EPOCH;
        let line = "123abc 456def mybranch";
        assert_eq!(
            parse_reference_transaction_line(timestamp, &line)?,
            Some(Event::RefUpdateEvent {
                timestamp: 0.0,
                old_ref: Some(String::from("123abc")),
                new_ref: Some(String::from("456def")),
                ref_name: String::from("mybranch"),
                message: None,
            })
        );

        let line = "123abc 456def ORIG_HEAD";
        assert_eq!(parse_reference_transaction_line(timestamp, &line)?, None);

        let line = "there are not three fields here";
        assert!(parse_reference_transaction_line(timestamp, &line).is_err());

        Ok(())
    }
}
