//! Amend the current commit.
//!
//! This command amends the HEAD commit with changes to files
//! that are already tracked in the repo. Following the amend,
//! the command performs a restack.

use std::fmt::Write;
use std::time::SystemTime;

use eyre::Context;
use itertools::Itertools;
use lib::core::rewrite::MergeConflictRemediation;
use tracing::instrument;

use crate::commands::restack;
use crate::opts::MoveOptions;
use lib::core::config::get_restack_preserve_timestamps;
use lib::core::effects::Effects;
use lib::core::eventlog::{Event, EventLogDb};
use lib::core::formatting::Pluralize;
use lib::core::gc::mark_commit_reachable;
use lib::git::{AmendFastOptions, FileStatus, GitRunInfo, Repo};

/// Amends the existing HEAD commit.
#[instrument]
pub fn amend(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    move_options: &MoveOptions,
) -> eyre::Result<isize> {
    let now = SystemTime::now();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let mut event_log_db = EventLogDb::new(&conn)?;

    let head_oid = match repo.get_head_info()?.oid {
        Some(oid) => oid,
        None => {
            writeln!(
                effects.get_output_stream(),
                "No commit is currently checked out. Check out a commit to amend and then try again.",
            )?;
            return Ok(1);
        }
    };
    let head_commit = repo.find_commit_or_fail(head_oid)?;

    let index = repo.get_index()?;
    if index.has_conflicts() {
        writeln!(
            effects.get_output_stream(),
            "Cannot amend, because there are unresolved merge conflicts. Resolve the merge conflicts and try again."
        )?;
        return Ok(1);
    }

    let event_tx_id = event_log_db.make_transaction_id(now, "amend")?;
    let staged_index_paths = repo.get_staged_paths()?;
    let (opts, dirty_working_tree) = if !staged_index_paths.is_empty() {
        let dirty_working_tree = repo.has_changed_files(effects, git_run_info)?;
        let opts = AmendFastOptions::FromIndex {
            paths: staged_index_paths.into_iter().collect_vec(),
        };
        (opts, dirty_working_tree)
    } else {
        let status = repo.get_status(git_run_info, Some(event_tx_id))?;
        let entries_to_amend = status
            .into_iter()
            .filter(|entry| match entry.working_copy_status {
                FileStatus::Added
                | FileStatus::Copied
                | FileStatus::Deleted
                | FileStatus::Modified
                | FileStatus::Renamed => true,
                FileStatus::Ignored
                | FileStatus::Unmerged
                | FileStatus::Unmodified
                | FileStatus::Untracked => false,
            })
            .collect_vec();
        let opts = AmendFastOptions::FromWorkingCopy {
            status_entries: entries_to_amend,
        };
        (opts, true)
    };
    if opts.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "There are no uncommitted or staged changes. Nothing to amend."
        )?;
        return Ok(0);
    }

    let amended_tree = repo.amend_fast(&head_commit, &opts)?;
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();

    let (author, committer) = (head_commit.get_author(), head_commit.get_committer());
    let (author, committer) = if get_restack_preserve_timestamps(&repo)? {
        (author, committer)
    } else {
        (
            author.update_timestamp(now)?,
            committer.update_timestamp(now)?,
        )
    };

    let amended_commit_oid = head_commit.amend_commit(
        Some("HEAD"),
        Some(&author),
        Some(&committer),
        None,
        Some(&amended_tree),
    )?;
    mark_commit_reachable(&repo, amended_commit_oid)
        .wrap_err("Marking commit as reachable for GC purposes.")?;

    event_log_db.add_events(vec![Event::RewriteEvent {
        timestamp,
        event_tx_id,
        old_commit_oid: head_oid.into(),
        new_commit_oid: amended_commit_oid.into(),
    }])?;

    if let AmendFastOptions::FromWorkingCopy { .. } = opts {
        // TODO(#201): Figure out a way to perform "fast amend" on the working copy without needing a reset.
        git_run_info.run(effects, Some(event_tx_id), &["reset"])?;
    }

    let restack_exit_code = restack::restack(
        effects,
        git_run_info,
        vec![head_oid.to_string()],
        move_options,
        MergeConflictRemediation::Restack,
    )?;
    if restack_exit_code != 0 {
        return Ok(restack_exit_code);
    }

    match opts {
        AmendFastOptions::FromIndex { paths } => {
            let staged_changes = Pluralize {
                determiner: None,
                amount: paths.len(),
                unit: ("staged change", "staged changes"),
            };
            let mut message = format!("Amended with {}.", staged_changes);
            // TODO: Include the number of uncommitted changes.
            if dirty_working_tree {
                message += " (Some uncommitted changes were not amended.)";
            }
            writeln!(effects.get_output_stream(), "{}", message)?;
        }
        AmendFastOptions::FromWorkingCopy { status_entries } => {
            let uncommitted_changes = Pluralize {
                determiner: None,
                amount: status_entries.len(),
                unit: ("uncommitted change", "uncommitted changes"),
            };
            writeln!(
                effects.get_output_stream(),
                "Amended with {}.",
                uncommitted_changes,
            )?;
        }
    }
    Ok(0)
}
