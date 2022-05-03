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
use lib::util::ExitCode;
use tracing::instrument;

use crate::commands::restack;
use crate::opts::MoveOptions;
use lib::core::config::get_restack_preserve_timestamps;
use lib::core::effects::Effects;
use lib::core::eventlog::{Event, EventLogDb};
use lib::core::formatting::Pluralize;
use lib::core::gc::mark_commit_reachable;
use lib::git::{AmendFastOptions, GitRunInfo, Repo};

/// Amends the existing HEAD commit.
#[instrument]
pub fn amend(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    move_options: &MoveOptions,
) -> eyre::Result<ExitCode> {
    let now = SystemTime::now();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;

    let head_info = repo.get_head_info()?;
    let head_oid = match head_info.oid {
        Some(oid) => oid,
        None => {
            writeln!(
                effects.get_output_stream(),
                "No commit is currently checked out. Check out a commit to amend and then try again.",
            )?;
            return Ok(ExitCode(1));
        }
    };
    let head_commit = repo.find_commit_or_fail(head_oid)?;

    let index = repo.get_index()?;
    if index.has_conflicts() {
        writeln!(
            effects.get_output_stream(),
            "Cannot amend, because there are unresolved merge conflicts. Resolve the merge conflicts and try again."
        )?;
        return Ok(ExitCode(1));
    }

    let event_tx_id = event_log_db.make_transaction_id(now, "amend")?;
    let (_snapshot, status) =
        repo.get_status(git_run_info, &index, &head_info, Some(event_tx_id))?;

    // Note that there may be paths which are in both of these entries in the
    // case that the given path has both staged and unstaged changes.
    let staged_entries = status
        .clone()
        .into_iter()
        .filter(|entry| entry.index_status.is_changed())
        .collect_vec();
    let unstaged_entries = status
        .into_iter()
        .filter(|entry| entry.working_copy_status.is_changed())
        .collect_vec();

    let opts = if !staged_entries.is_empty() {
        AmendFastOptions::FromIndex {
            paths: staged_entries
                .into_iter()
                .flat_map(|entry| entry.paths())
                .collect(),
        }
    } else {
        AmendFastOptions::FromWorkingCopy {
            status_entries: unstaged_entries.clone(),
        }
    };
    if opts.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "There are no uncommitted or staged changes. Nothing to amend."
        )?;
        return Ok(ExitCode(0));
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
        let exit_code = git_run_info.run(effects, Some(event_tx_id), &["reset"])?;
        if !exit_code.is_success() {
            return Ok(exit_code);
        }
    }

    let restack_exit_code = restack::restack(
        effects,
        git_run_info,
        vec![head_oid.to_string()],
        move_options,
        MergeConflictRemediation::Restack,
    )?;
    if !restack_exit_code.is_success() {
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
            if !unstaged_entries.is_empty() {
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
    Ok(ExitCode(0))
}
