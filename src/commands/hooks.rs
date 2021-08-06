//! Callbacks for Git hooks.
//!
//! Git uses "hooks" to run user-defined scripts after certain events. We
//! extensively use these hooks to track user activity and e.g. decide if a
//! commit should be considered "hidden".
//!
//! The hooks are installed by the `branchless init` command. This module
//! contains the implementations for the hooks.

use std::convert::TryInto;
use std::ffi::OsString;
use std::io::{stdin, BufRead, Cursor};
use std::time::SystemTime;

use eyre::Context;
use itertools::Itertools;
use os_str_bytes::OsStringBytes;
use tracing::{error, instrument, warn};

use crate::commands::gc::mark_commit_reachable;
use crate::core::eventlog::{should_ignore_ref_updates, Event, EventLogDb, EventTransactionId};
use crate::core::formatting::{printable_styled_string, Glyphs, Pluralize};
use crate::git::{CategorizedReferenceName, MaybeZeroOid, Repo};

pub use crate::core::rewrite::hooks::{
    hook_drop_commit_if_empty, hook_post_rewrite, hook_register_extra_post_rewrite_hook,
    hook_skip_upstream_applied_commit,
};

/// Handle Git's `post-checkout` hook.
///
/// See the man-page for `githooks(5)`.
#[instrument]
pub fn hook_post_checkout(
    previous_head_oid: &str,
    current_head_oid: &str,
    is_branch_checkout: isize,
) -> eyre::Result<()> {
    if is_branch_checkout == 0 {
        return Ok(());
    }

    let now = SystemTime::now();
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?;
    println!("branchless: processing checkout");

    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let mut event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "hook-post-checkout")?;
    event_log_db.add_events(vec![Event::RefUpdateEvent {
        timestamp: timestamp.as_secs_f64(),
        event_tx_id,
        old_oid: previous_head_oid.parse()?,
        new_oid: {
            let oid: MaybeZeroOid = current_head_oid.parse()?;
            oid
        },
        ref_name: OsString::from("HEAD"),
        message: None,
    }])?;
    Ok(())
}

/// Handle Git's `post-commit` hook.
///
/// See the man-page for `githooks(5)`.
#[instrument]
pub fn hook_post_commit() -> eyre::Result<()> {
    let now = SystemTime::now();
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let mut event_log_db = EventLogDb::new(&conn)?;

    let commit_oid = match repo.get_head_info()?.oid {
        Some(commit_oid) => commit_oid,
        None => {
            // A strange situation, but technically possible.
            warn!("`post-commit` hook called, but could not determine the OID of `HEAD`");
            return Ok(());
        }
    };

    let commit = match repo.find_commit(commit_oid)? {
        Some(commit) => commit,
        None => {
            eyre::bail!(
                "BUG: Attempted to look up current `HEAD` commit, but it could not be found: {:?}",
                commit_oid
            )
        }
    };
    mark_commit_reachable(&repo, commit_oid)
        .wrap_err_with(|| "Marking commit as reachable for GC purposes")?;

    let timestamp = commit.get_time().seconds() as f64;
    let event_tx_id = event_log_db.make_transaction_id(now, "hook-post-commit")?;
    event_log_db.add_events(vec![Event::CommitEvent {
        timestamp,
        event_tx_id,
        commit_oid: commit.get_oid(),
    }])?;
    println!(
        "branchless: processed commit: {}",
        printable_styled_string(&glyphs, commit.friendly_describe()?)?,
    );

    Ok(())
}

#[instrument]
fn parse_reference_transaction_line(
    line: &[u8],
    now: SystemTime,
    event_tx_id: EventTransactionId,
) -> eyre::Result<Option<Event>> {
    let cursor = Cursor::new(line);
    let fields = {
        let mut fields = Vec::new();
        for field in cursor.split(b' ') {
            let field = field.wrap_err_with(|| "Reading reference-transaction field")?;
            let field = OsString::from_raw_vec(field)
                .wrap_err_with(|| "Decoding reference-transaction field")?;
            fields.push(field);
        }
        fields
    };
    match fields.as_slice() {
        [old_value, new_value, ref_name] => {
            if !should_ignore_ref_updates(ref_name) {
                let timestamp = now
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .wrap_err_with(|| "Processing timestamp")?;
                Ok(Some(Event::RefUpdateEvent {
                    timestamp: timestamp.as_secs_f64(),
                    event_tx_id,
                    ref_name: ref_name.clone(),
                    old_oid: old_value.as_os_str().try_into()?,
                    new_oid: {
                        let oid: MaybeZeroOid = new_value.as_os_str().try_into()?;
                        oid
                    },
                    message: None,
                }))
            } else {
                Ok(None)
            }
        }
        _ => {
            eyre::bail!(
                "Unexpected number of fields in reference-transaction line: {:?}",
                &line
            )
        }
    }
}

/// Handle Git's `reference-transaction` hook.
///
/// See the man-page for `githooks(5)`.
#[instrument]
pub fn hook_reference_transaction(transaction_state: &str) -> eyre::Result<()> {
    if transaction_state != "committed" {
        return Ok(());
    }
    let now = SystemTime::now();

    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let mut event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "reference-transaction")?;

    let events: Vec<Event> = stdin()
        .lock()
        .split(b'\n')
        .filter_map(|line| {
            let line = match line {
                Ok(line) => line,
                Err(_) => return None,
            };
            match parse_reference_transaction_line(line.as_slice(), now, event_tx_id) {
                Ok(event) => event,
                Err(err) => {
                    error!(?err, "Could not parse reference-transaction-line");
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
        singular: "update",
        plural: "updates",
    };
    println!(
        "branchless: processing {}: {}",
        num_reference_updates.to_string(),
        events
            .iter()
            .filter_map(|event| {
                match event {
                    Event::RefUpdateEvent { ref_name, .. } => {
                        Some(CategorizedReferenceName::new(ref_name).friendly_describe())
                    }
                    Event::RewriteEvent { .. }
                    | Event::CommitEvent { .. }
                    | Event::HideEvent { .. }
                    | Event::UnhideEvent { .. } => None,
                }
            })
            .map(|description| format!("{}", console::style(description).green()))
            .sorted()
            .collect::<Vec<_>>()
            .join(", ")
    );
    event_log_db.add_events(events)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::testing::{make_git, GitRunOptions};

    use super::*;

    #[test]
    fn test_parse_reference_transaction_line() -> eyre::Result<()> {
        let line = b"123abc 456def refs/heads/mybranch";
        let timestamp = SystemTime::UNIX_EPOCH;
        let event_tx_id = crate::core::eventlog::testing::make_dummy_transaction_id(789);
        assert_eq!(
            parse_reference_transaction_line(line, timestamp, event_tx_id)?,
            Some(Event::RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id,
                old_oid: "123abc".parse()?,
                new_oid: {
                    let oid: MaybeZeroOid = "456def".parse()?;
                    oid.into()
                },
                ref_name: OsString::from("refs/heads/mybranch"),
                message: None,
            })
        );

        let line = b"123abc 456def ORIG_HEAD";
        assert_eq!(
            parse_reference_transaction_line(line, timestamp, event_tx_id)?,
            None
        );

        let line = b"there are not three fields here";
        assert!(parse_reference_transaction_line(line, timestamp, event_tx_id).is_err());

        Ok(())
    }

    #[test]
    fn test_is_rebase_underway() -> eyre::Result<()> {
        let git = make_git()?;

        git.init_repo()?;
        let repo = git.get_repo()?;
        assert!(!repo.is_rebase_underway()?);

        let oid1 = git.commit_file_with_contents("test", 1, "foo")?;
        git.run(&["checkout", "HEAD^"])?;
        git.commit_file_with_contents("test", 1, "bar")?;
        git.run_with_options(
            &["rebase", &oid1.to_string()],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        assert!(repo.is_rebase_underway()?);

        Ok(())
    }
}
