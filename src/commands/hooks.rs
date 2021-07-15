//! Callbacks for Git hooks.
//!
//! Git uses "hooks" to run user-defined scripts after certain events. We
//! extensively use these hooks to track user activity and e.g. decide if a
//! commit should be considered "hidden".
//!
//! The hooks are installed by the `branchless init` command. This module
//! contains the implementations for the hooks.

use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{stdin, BufRead, Cursor};
use std::time::SystemTime;

use anyhow::Context;
use console::style;
use fn_error_context::context;
use itertools::Itertools;
use os_str_bytes::OsStringBytes;

use crate::commands::gc::mark_commit_reachable;
use crate::core::config::{get_restack_warn_abandoned, RESTACK_WARN_ABANDONED_CONFIG_KEY};
use crate::core::eventlog::{
    should_ignore_ref_updates, Event, EventLogDb, EventReplayer, EventTransactionId,
};
use crate::core::formatting::{printable_styled_string, Glyphs, Pluralize};
use crate::core::graph::{make_graph, BranchOids, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::core::rewrite::{find_abandoned_children, move_branches};
use crate::git::{CategorizedReferenceName, GitRunInfo, Oid, Repo};

const EXTRA_POST_REWRITE_FILE_NAME: &str = "branchless_do_extra_post_rewrite";

/// Handle Git's `post-rewrite` hook.
///
/// See the man-page for `githooks(5)`.
#[context("Processing post-rewrite hook")]
pub fn hook_post_rewrite(git_run_info: &GitRunInfo, rewrite_type: &str) -> anyhow::Result<()> {
    let now = SystemTime::now();
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();

    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let mut event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "hook-post-rewrite")?;

    let (rewritten_oids, events) = {
        let mut rewritten_oids = HashMap::new();
        let mut events = Vec::new();
        for line in stdin().lock().lines() {
            let line = line?;
            let line = line.trim();
            match *line.split(' ').collect::<Vec<_>>().as_slice() {
                [old_commit_oid, new_commit_oid, ..] => {
                    let old_commit_oid: Oid = old_commit_oid.parse()?;
                    let new_commit_oid: Oid = new_commit_oid.parse()?;

                    rewritten_oids.insert(old_commit_oid, new_commit_oid);
                    events.push(Event::RewriteEvent {
                        timestamp,
                        event_tx_id,
                        old_commit_oid,
                        new_commit_oid,
                    })
                }
                _ => anyhow::bail!("Invalid rewrite line: {:?}", &line),
            }
        }
        (rewritten_oids, events)
    };

    let is_spurious_event = rewrite_type == "amend" && repo.is_rebase_underway()?;
    if !is_spurious_event {
        let message_rewritten_commits = Pluralize {
            amount: events.len().try_into()?,
            singular: "rewritten commit",
            plural: "rewritten commits",
        }
        .to_string();
        println!("branchless: processing {}", message_rewritten_commits);
    }

    event_log_db.add_events(events)?;

    if repo
        .get_rebase_state_dir_path()
        .join(EXTRA_POST_REWRITE_FILE_NAME)
        .exists()
    {
        move_branches(git_run_info, &repo, event_tx_id, &rewritten_oids)?;
    }

    let should_check_abandoned_commits = get_restack_warn_abandoned(&repo)?;
    if should_check_abandoned_commits && !is_spurious_event {
        let merge_base_db = MergeBaseDb::new(&conn)?;
        warn_abandoned(
            &repo,
            &merge_base_db,
            &event_log_db,
            rewritten_oids.keys().copied(),
        )?;
    }

    Ok(())
}

#[context("Warning about abandoned commits/branches")]
fn warn_abandoned(
    repo: &Repo,
    merge_base_db: &MergeBaseDb,
    event_log_db: &EventLogDb,
    old_commit_oids: impl IntoIterator<Item = Oid>,
) -> anyhow::Result<()> {
    // The caller will have added events to the event log database, so make sure
    // to construct a fresh `EventReplayer` here.
    let event_replayer = EventReplayer::from_event_log_db(repo, event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();

    let head_oid = repo.get_head_info()?.oid;
    let main_branch_oid = repo.get_main_branch_oid()?;
    let branch_oid_to_names = repo.get_branch_oid_to_names()?;
    let graph = make_graph(
        &repo,
        &merge_base_db,
        &event_replayer,
        event_cursor,
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        false,
    )?;

    let (all_abandoned_children, all_abandoned_branches) = {
        let mut all_abandoned_children: HashSet<Oid> = HashSet::new();
        let mut all_abandoned_branches: HashSet<&OsStr> = HashSet::new();
        for old_commit_oid in old_commit_oids {
            let abandoned_result = find_abandoned_children(
                &graph,
                &event_replayer,
                event_replayer.make_default_cursor(),
                old_commit_oid,
            );
            let (_rewritten_oid, abandoned_children) = match abandoned_result {
                Some(abandoned_result) => abandoned_result,
                None => continue,
            };
            all_abandoned_children.extend(abandoned_children.iter());
            if let Some(branch_names) = branch_oid_to_names.get(&old_commit_oid) {
                all_abandoned_branches.extend(branch_names.iter().map(OsString::as_os_str));
            }
        }
        (all_abandoned_children, all_abandoned_branches)
    };
    let num_abandoned_children = all_abandoned_children.len();
    let num_abandoned_branches = all_abandoned_branches.len();

    if num_abandoned_children > 0 || num_abandoned_branches > 0 {
        let warning_items = {
            let mut warning_items = Vec::new();
            if num_abandoned_children > 0 {
                warning_items.push(
                    Pluralize {
                        amount: num_abandoned_children.try_into()?,
                        singular: "commit",
                        plural: "commits",
                    }
                    .to_string(),
                );
            }
            if num_abandoned_branches > 0 {
                let abandoned_branch_count = Pluralize {
                    amount: num_abandoned_branches.try_into()?,
                    singular: "branch",
                    plural: "branches",
                }
                .to_string();

                let mut all_abandoned_branches: Vec<String> = all_abandoned_branches
                    .iter()
                    .map(|branch_name| CategorizedReferenceName::new(branch_name).render_suffix())
                    .collect();
                all_abandoned_branches.sort_unstable();
                let abandoned_branches_list = all_abandoned_branches.join(", ");
                warning_items.push(format!(
                    "{} ({})",
                    abandoned_branch_count, abandoned_branches_list
                ));
            }

            warning_items
        };

        let warning_message = warning_items.join(" and ");
        let warning_message = style(format!("This operation abandoned {}!", warning_message))
            .bold()
            .yellow();

        print!(
            "\
branchless: {warning_message}
branchless: Consider running one of the following:
branchless:   - {git_restack}: re-apply the abandoned commits/branches
branchless:     (this is most likely what you want to do)
branchless:   - {git_smartlog}: assess the situation
branchless:   - {git_hide} [<commit>...]: hide the commits from the smartlog
branchless:   - {git_undo}: undo the operation
branchless:   - {config_command}: suppress this message
",
            warning_message = warning_message,
            git_smartlog = style("git smartlog").bold(),
            git_restack = style("git restack").bold(),
            git_hide = style("git hide").bold(),
            git_undo = style("git undo").bold(),
            config_command = style(format!(
                "git config {} false",
                RESTACK_WARN_ABANDONED_CONFIG_KEY
            ))
            .bold(),
        );
    }

    Ok(())
}

/// For rebases, register that extra cleanup actions should be taken when the
/// rebase finishes and calls the post-rewrite hook. We don't want to change the
/// behavior of `git rebase` itself, except when called via `git-branchless`, so
/// that the user's expectations aren't unexpectedly subverted.
pub fn hook_register_extra_post_rewrite_hook() -> anyhow::Result<()> {
    let repo = Repo::from_current_dir()?;
    let file_name = repo
        .get_rebase_state_dir_path()
        .join(EXTRA_POST_REWRITE_FILE_NAME);
    File::create(file_name).with_context(|| "Registering extra post-rewrite hook")?;
    Ok(())
}

/// Handle Git's `post-checkout` hook.
///
/// See the man-page for `githooks(5)`.
#[context("Processing post-checkout hook")]
pub fn hook_post_checkout(
    previous_head_ref: &str,
    current_head_ref: &str,
    is_branch_checkout: isize,
) -> anyhow::Result<()> {
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
        old_ref: Some(OsString::from(previous_head_ref)),
        new_ref: Some(OsString::from(current_head_ref)),
        ref_name: OsString::from("HEAD"),
        message: None,
    }])?;
    Ok(())
}

/// Handle Git's `post-commit` hook.
///
/// See the man-page for `githooks(5)`.
pub fn hook_post_commit() -> anyhow::Result<()> {
    let now = SystemTime::now();
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let mut event_log_db = EventLogDb::new(&conn)?;

    let commit_oid = match repo.get_head_info()?.oid {
        Some(commit_oid) => commit_oid,
        None => {
            // A strange situation, but technically possible.
            log::warn!("`post-commit` hook called, but could not determine the OID of `HEAD`");
            return Ok(());
        }
    };

    let commit = match repo.find_commit(commit_oid)? {
        Some(commit) => commit,
        None => {
            anyhow::bail!(
                "BUG: Attempted to look up current `HEAD` commit, but it could not be found: {:?}",
                commit_oid
            )
        }
    };
    mark_commit_reachable(&repo, commit_oid)
        .with_context(|| "Marking commit as reachable for GC purposes")?;

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

fn parse_reference_transaction_line(
    line: &[u8],
    now: SystemTime,
    event_tx_id: EventTransactionId,
) -> anyhow::Result<Option<Event>> {
    let cursor = Cursor::new(line);
    let fields = {
        let mut fields = Vec::new();
        for field in cursor.split(b' ') {
            let field = field.with_context(|| "Reading reference-transaction field")?;
            let field = OsString::from_raw_vec(field)
                .with_context(|| "Decoding reference-transaction field")?;
            fields.push(field);
        }
        fields
    };
    match fields.as_slice() {
        [old_value, new_value, ref_name] => {
            if !should_ignore_ref_updates(ref_name) {
                let timestamp = now
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .with_context(|| "Processing timestamp")?;
                Ok(Some(Event::RefUpdateEvent {
                    timestamp: timestamp.as_secs_f64(),
                    event_tx_id,
                    ref_name: ref_name.clone(),
                    old_ref: Some(old_value.clone()),
                    new_ref: Some(new_value.clone()),
                    message: None,
                }))
            } else {
                Ok(None)
            }
        }
        _ => {
            anyhow::bail!(
                "Unexpected number of fields in reference-transaction line: {:?}",
                &line
            )
        }
    }
}

/// Handle Git's `reference-transaction` hook.
///
/// See the man-page for `githooks(5)`.
#[context("Processing reference-transaction hook")]
pub fn hook_reference_transaction(transaction_state: &str) -> anyhow::Result<()> {
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
    fn test_parse_reference_transaction_line() -> anyhow::Result<()> {
        let line = b"123abc 456def refs/heads/mybranch";
        let timestamp = SystemTime::UNIX_EPOCH;
        let event_tx_id = crate::core::eventlog::testing::make_dummy_transaction_id(789);
        assert_eq!(
            parse_reference_transaction_line(line, timestamp, event_tx_id)?,
            Some(Event::RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id,
                old_ref: Some(OsString::from("123abc")),
                new_ref: Some(OsString::from("456def")),
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
    fn test_is_rebase_underway() -> anyhow::Result<()> {
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
