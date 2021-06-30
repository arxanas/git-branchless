//! Callbacks for Git hooks.
//!
//! Git uses "hooks" to run user-defined scripts after certain events. We
//! extensively use these hooks to track user activity and e.g. decide if a
//! commit should be considered "hidden".
//!
//! The hooks are installed by the `branchless init` command. This module
//! contains the implementations for the hooks.

use std::collections::HashSet;
use std::convert::TryInto;
use std::io::{stdin, BufRead};
use std::time::SystemTime;

use anyhow::Context;
use console::style;
use fn_error_context::context;

use crate::commands::gc::mark_commit_reachable;
use crate::core::config::{get_restack_warn_abandoned, RESTACK_WARN_ABANDONED_CONFIG_KEY};
use crate::core::eventlog::{
    should_ignore_ref_updates, Event, EventLogDb, EventReplayer, EventTransactionId,
};
use crate::core::formatting::Pluralize;
use crate::core::graph::{make_graph, BranchOids, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::core::repo::Repo;
use crate::core::rewrite::find_abandoned_children;
use crate::util::{get_branch_oid_to_names, get_db_conn, get_head_oid, get_main_branch_oid};

/// Detect if an interactive rebase has started but not completed.
///
/// Git will send us spurious `post-rewrite` events marked as `amend` during an
/// interactive rebase, indicating that some of the commits have been rewritten
/// as part of the rebase plan, but not all of them. This function attempts to
/// detect when an interactive rebase is underway, and if the current
/// `post-rewrite` event is spurious.
///
/// There are two practical issues for users as a result of this Git behavior:
///
///   * During an interactive rebase, we may see many "processing 1 rewritten
///   commit" messages, and then a final "processing X rewritten commits" message
///   once the rebase has concluded. This is potentially confusing for users, since
///   the operation logically only rewrote the commits once, but we displayed the
///   message multiple times.
///
///   * During an interactive rebase, we may warn about abandoned commits, when the
///   next operation in the rebase plan fixes up the abandoned commit. This can
///   happen even if no conflict occurred and the rebase completed successfully
///   without any user intervention.
#[context("Determining if rebase is underway")]
fn is_rebase_underway(repo: &git2::Repository) -> anyhow::Result<bool> {
    match repo.state() {
        git2::RepositoryState::Rebase
        | git2::RepositoryState::RebaseInteractive
        | git2::RepositoryState::RebaseMerge => Ok(true),

        // Possibly some of these states should also be treated as `true`?
        git2::RepositoryState::Clean
        | git2::RepositoryState::Merge
        | git2::RepositoryState::Revert
        | git2::RepositoryState::RevertSequence
        | git2::RepositoryState::CherryPick
        | git2::RepositoryState::CherryPickSequence
        | git2::RepositoryState::Bisect
        | git2::RepositoryState::ApplyMailbox
        | git2::RepositoryState::ApplyMailboxOrRebase => Ok(false),
    }
}

/// Handle Git's `post-rewrite` hook.
///
/// See the man-page for `githooks(5)`.
#[context("Processing post-rewrite hook")]
pub fn hook_post_rewrite(rewrite_type: &str) -> anyhow::Result<()> {
    let now = SystemTime::now();
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();

    let repo = Repo::from_current_dir()?;
    let conn = get_db_conn(&repo)?;
    let mut event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "hook-post-rewrite")?;

    let (old_commits, events) = {
        let mut old_commits = Vec::new();
        let mut events = Vec::new();
        for line in stdin().lock().lines() {
            let line = line?;
            let line = line.trim();
            match *line.split(' ').collect::<Vec<_>>().as_slice() {
                [old_commit_oid, new_commit_oid, ..] => {
                    let old_commit_oid =
                        git2::Oid::from_str(old_commit_oid).with_context(|| {
                            format!("Could not convert {:?} to OID", old_commit_oid)
                        })?;
                    let new_commit_oid =
                        git2::Oid::from_str(new_commit_oid).with_context(|| {
                            format!("Could not convert {:?} to OID", new_commit_oid)
                        })?;

                    old_commits.push(old_commit_oid);
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
        (old_commits, events)
    };

    let is_spurious_event = rewrite_type == "amend" && is_rebase_underway(&repo)?;
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

    let should_check_abandoned_commits = get_restack_warn_abandoned(&repo)?;
    if is_spurious_event || !should_check_abandoned_commits {
        return Ok(());
    }

    let merge_base_db = MergeBaseDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
    let head_oid = get_head_oid(&repo)?;
    let main_branch_oid = get_main_branch_oid(&repo)?;
    let branch_oid_to_names = get_branch_oid_to_names(&repo)?;
    let graph = make_graph(
        &repo,
        &merge_base_db,
        &event_replayer,
        event_replayer.make_default_cursor(),
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        false,
    )?;

    let (all_abandoned_children, all_abandoned_branches) = {
        let mut all_abandoned_children: HashSet<git2::Oid> = HashSet::new();
        let mut all_abandoned_branches: HashSet<&str> = HashSet::new();
        for old_commit_oid in old_commits {
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
                all_abandoned_branches.extend(branch_names.iter().map(String::as_str));
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

                let mut all_abandoned_branches: Vec<&str> =
                    all_abandoned_branches.iter().copied().collect();
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
    let conn = get_db_conn(&repo)?;
    let mut event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "hook-post-checkout")?;
    event_log_db.add_events(vec![Event::RefUpdateEvent {
        timestamp: timestamp.as_secs_f64(),
        event_tx_id,
        old_ref: Some(String::from(previous_head_ref)),
        new_ref: Some(String::from(current_head_ref)),
        ref_name: String::from("HEAD"),
        message: None,
    }])?;
    Ok(())
}

/// Handle Git's `post-commit` hook.
///
/// See the man-page for `githooks(5)`.
pub fn hook_post_commit() -> anyhow::Result<()> {
    println!("branchless: processing commit");

    let now = SystemTime::now();
    let repo = Repo::from_current_dir()?;
    let conn = get_db_conn(&repo)?;
    let mut event_log_db = EventLogDb::new(&conn)?;

    let commit = repo
        .head()
        .with_context(|| "Getting repo HEAD")?
        .peel_to_commit()
        .with_context(|| "Getting HEAD commit")?;
    mark_commit_reachable(&repo, commit.id())
        .with_context(|| "Marking commit as reachable for GC purposes")?;

    let timestamp = commit.time().seconds() as f64;
    let event_tx_id = event_log_db.make_transaction_id(now, "hook-post-commit")?;
    event_log_db.add_events(vec![Event::CommitEvent {
        timestamp,
        event_tx_id,
        commit_oid: commit.id(),
    }])?;

    Ok(())
}

fn parse_reference_transaction_line(
    line: &str,
    now: SystemTime,
    event_tx_id: EventTransactionId,
) -> anyhow::Result<Option<Event>> {
    match *line.split(' ').collect::<Vec<_>>().as_slice() {
        [old_value, new_value, ref_name] => {
            if !should_ignore_ref_updates(ref_name) {
                let timestamp = now
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .with_context(|| "Processing timestamp")?;
                Ok(Some(Event::RefUpdateEvent {
                    timestamp: timestamp.as_secs_f64(),
                    event_tx_id,
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
#[context("Processing reference-transaction hook")]
pub fn hook_reference_transaction(transaction_state: &str) -> anyhow::Result<()> {
    if transaction_state != "committed" {
        return Ok(());
    }
    let now = SystemTime::now();

    let repo = Repo::from_current_dir()?;
    let conn = get_db_conn(&repo)?;
    let mut event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "reference-transaction")?;

    let events: Vec<Event> = stdin()
        .lock()
        .lines()
        .filter_map(|line| {
            let line = match line {
                Ok(line) => line,
                Err(_) => return None,
            };
            match parse_reference_transaction_line(&line, now, event_tx_id) {
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
    println!(
        "branchless: processing {}",
        num_reference_updates.to_string()
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
        let line = "123abc 456def mybranch";
        let timestamp = SystemTime::UNIX_EPOCH;
        let event_tx_id = crate::core::eventlog::testing::make_dummy_transaction_id(789);
        assert_eq!(
            parse_reference_transaction_line(&line, timestamp, event_tx_id)?,
            Some(Event::RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id,
                old_ref: Some(String::from("123abc")),
                new_ref: Some(String::from("456def")),
                ref_name: String::from("mybranch"),
                message: None,
            })
        );

        let line = "123abc 456def ORIG_HEAD";
        assert_eq!(
            parse_reference_transaction_line(&line, timestamp, event_tx_id)?,
            None
        );

        let line = "there are not three fields here";
        assert!(parse_reference_transaction_line(&line, timestamp, event_tx_id).is_err());

        Ok(())
    }

    #[test]
    fn test_is_rebase_underway() -> anyhow::Result<()> {
        let git = make_git()?;

        git.init_repo()?;
        let repo = git.get_repo()?;
        assert!(!is_rebase_underway(&repo)?);

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
        assert!(is_rebase_underway(&repo)?);

        Ok(())
    }
}
