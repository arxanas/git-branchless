//! Handle obsoleting commits when explicitly requested by the user (as opposed to
//! automatically as the result of a rewrite operation).

use std::collections::HashMap;
use std::fmt::Write;
use std::time::SystemTime;

use eden_dag::DagAlgorithm;
use lib::core::repo_ext::RepoExt;
use lib::util::ExitCode;
use tracing::instrument;

use lib::core::dag::{sorted_commit_set, union_all, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{CommitActivityStatus, Event};
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::{Glyphs, Pluralize};
use lib::core::rewrite::move_branches;
use lib::git::{CategorizedReferenceName, GitRunInfo, MaybeZeroOid, NonZeroOid, Repo};

use crate::opts::{ResolveRevsetOptions, Revset};
use crate::revset::resolve_commits;

/// Hide the hashes provided on the command-line.
#[instrument]
pub fn hide(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    revsets: Vec<Revset>,
    resolve_revset_options: &ResolveRevsetOptions,
    delete_branches: bool,
    recursive: bool,
) -> eyre::Result<ExitCode> {
    let now = SystemTime::now();
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let commit_sets =
        match resolve_commits(effects, &repo, &mut dag, &revsets, resolve_revset_options) {
            Ok(commit_sets) => commit_sets,
            Err(err) => {
                err.describe(effects)?;
                return Ok(ExitCode(1));
            }
        };

    let commits = union_all(&commit_sets);
    let commits = if recursive {
        dag.filter_visible_commits(dag.query().descendants(commits)?)?
    } else {
        commits
    };
    let commits = dag.query().sort(&commits)?;
    let commits = sorted_commit_set(&repo, &dag, &commits)?;

    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();
    let event_tx_id = event_log_db.make_transaction_id(now, "hide")?;
    let events = commits
        .iter()
        .map(|commit| Event::ObsoleteEvent {
            timestamp,
            event_tx_id,
            commit_oid: commit.get_oid(),
        })
        .collect();
    event_log_db.add_events(events)?;

    let cursor = event_replayer.make_default_cursor();
    let num_commits = commits.len();
    for commit in commits.iter() {
        writeln!(
            effects.get_output_stream(),
            "Hid commit: {}",
            glyphs.render(commit.friendly_describe(&glyphs)?)?,
        )?;
        if let CommitActivityStatus::Obsolete =
            event_replayer.get_cursor_commit_activity_status(cursor, commit.get_oid())
        {
            writeln!(
                effects.get_output_stream(),
                "(It was already hidden, so this operation had no effect.)"
            )?;
        }
    }

    if delete_branches {
        // Save current HEAD info *before* deleting any branches.
        let head_info = repo.get_head_info()?;

        // Delete any branches pointing to any of the hidden commits by "moving" them from their
        // current OID to a Zero OID.
        let abandoned_branches: HashMap<NonZeroOid, MaybeZeroOid> = commits
            .iter()
            .map(|commit| (commit.get_oid(), MaybeZeroOid::Zero))
            .collect();
        if let Some(head_oid) = head_info.oid {
            if abandoned_branches.contains_key(&head_oid) {
                repo.detach_head(&head_info)?;
            }
        }
        move_branches(
            effects,
            git_run_info,
            &repo,
            event_tx_id,
            &abandoned_branches,
        )?;
    }

    let mut abandoned_branches: Vec<String> = commits
        .iter()
        .filter_map(|commit| {
            references_snapshot
                .branch_oid_to_names
                .get(&commit.get_oid())
        })
        .flatten()
        .map(|branch_name| CategorizedReferenceName::new(branch_name).render_suffix())
        .collect();
    if !abandoned_branches.is_empty() {
        abandoned_branches.sort_unstable();
        // This message will look like either of these:
        // Abandoned X branches: <branches>
        // Deleted X branches: <branches>
        writeln!(
            effects.get_output_stream(),
            "{} {}: {}",
            if delete_branches {
                "Deleted"
            } else {
                "Abandoned"
            },
            Pluralize {
                determiner: None,
                amount: abandoned_branches.len(),
                unit: ("branch", "branches"),
            },
            abandoned_branches.join(", ")
        )?;
    }

    // This message will look like either of these:
    // To unhide these X commits, run: git undo
    // To unhide these X commits and restore X branches, run: git undo
    let delete_branches_message = match delete_branches {
        true => format!(
            " and restore {}",
            Pluralize {
                determiner: None,
                amount: abandoned_branches.len(),
                unit: ("branch", "branches"),
            }
        ),
        false => String::new(),
    };
    writeln!(
        effects.get_output_stream(),
        "To unhide {}{}, run: git undo",
        Pluralize {
            determiner: Some(("this", "these")),
            amount: num_commits,
            unit: ("commit", "commits"),
        },
        delete_branches_message
    )?;

    Ok(ExitCode(0))
}

/// Unhide the hashes provided on the command-line.
#[instrument]
pub fn unhide(
    effects: &Effects,
    revsets: Vec<Revset>,
    resolve_revset_options: &ResolveRevsetOptions,
    recursive: bool,
) -> eyre::Result<ExitCode> {
    let now = SystemTime::now();
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let commit_sets =
        match resolve_commits(effects, &repo, &mut dag, &revsets, resolve_revset_options) {
            Ok(commit_sets) => commit_sets,
            Err(err) => {
                err.describe(effects)?;
                return Ok(ExitCode(1));
            }
        };

    let commits = union_all(&commit_sets);
    let commits = if recursive {
        dag.query()
            .descendants(commits)?
            .intersection(&dag.query_obsolete_commits())
    } else {
        commits
    };
    let commits = dag.query().sort(&commits)?;
    let commits = sorted_commit_set(&repo, &dag, &commits)?;

    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();
    let event_tx_id = event_log_db.make_transaction_id(now, "unhide")?;
    let events = commits
        .iter()
        .map(|commit| Event::UnobsoleteEvent {
            timestamp,
            event_tx_id,
            commit_oid: commit.get_oid(),
        })
        .collect();
    event_log_db.add_events(events)?;

    let cursor = event_replayer.make_default_cursor();
    let num_commits = commits.len();
    for commit in commits {
        writeln!(
            effects.get_output_stream(),
            "Unhid commit: {}",
            glyphs.render(commit.friendly_describe(&glyphs)?)?,
        )?;
        if let CommitActivityStatus::Active =
            event_replayer.get_cursor_commit_activity_status(cursor, commit.get_oid())
        {
            writeln!(
                effects.get_output_stream(),
                "(It was not hidden, so this operation had no effect.)"
            )?;
        }
    }

    writeln!(
        effects.get_output_stream(),
        "To hide {}, run: git undo",
        Pluralize {
            determiner: Some(("this", "these")),
            amount: num_commits,
            unit: ("commit", "commits"),
        },
    )?;

    Ok(ExitCode(0))
}
