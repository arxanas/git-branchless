//! Handle obsoleting commits when explicitly requested by the user (as opposed to
//! automatically as the result of a rewrite operation).

use std::fmt::Write;
use std::time::SystemTime;

use eden_dag::DagAlgorithm;
use lib::core::repo_ext::RepoExt;
use lib::util::ExitCode;
use tracing::instrument;

use lib::core::dag::{sort_commit_set, CommitSet, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{CommitActivityStatus, Event};
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::{printable_styled_string, Glyphs, Pluralize};
use lib::core::rewrite::move_branches;
use lib::git::{CategorizedReferenceName, GitRunInfo, MaybeZeroOid, Repo};

use crate::revset::{resolve_commits, ResolveCommitsResult};

/// Hide the hashes provided on the command-line.
#[instrument]
pub fn hide(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    hashes: Vec<String>,
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

    let commits = resolve_commits(effects, &repo, &mut dag, hashes)?;
    let commits = match commits {
        ResolveCommitsResult::Ok { commits } => commits,
        ResolveCommitsResult::CommitNotFound { commit: hash } => {
            writeln!(effects.get_output_stream(), "Commit not found: {}", hash)?;
            return Ok(ExitCode(1));
        }
    };

    let commits: CommitSet = commits
        .into_iter()
        .map(|commit| commit.get_oid())
        .rev()
        .collect();
    let commits = if recursive {
        dag.query()
            .descendants(commits)?
            .difference(&dag.obsolete_commits)
    } else {
        commits
    };
    let commits = dag.query().sort(&commits)?;
    let commits = sort_commit_set(&repo, &dag, &commits)?;

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
            printable_styled_string(&glyphs, commit.friendly_describe(&glyphs)?)?,
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
        // Delete any branches pointing to any of the hidden commits by "moving" them from their
        // current OID to a Zero OID.
        let abandoned_branches = commits
            .iter()
            .map(|commit| (commit.get_oid(), MaybeZeroOid::Zero))
            .collect();
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
pub fn unhide(effects: &Effects, hashes: Vec<String>, recursive: bool) -> eyre::Result<ExitCode> {
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

    let commits = resolve_commits(effects, &repo, &mut dag, hashes)?;
    let commits = match commits {
        ResolveCommitsResult::Ok { commits } => commits,
        ResolveCommitsResult::CommitNotFound { commit: hash } => {
            writeln!(effects.get_output_stream(), "Commit not found: {}", hash)?;
            return Ok(ExitCode(1));
        }
    };

    let commits: CommitSet = commits.into_iter().map(|commit| commit.get_oid()).collect();
    let commits = if recursive {
        dag.query()
            .descendants(commits)?
            .intersection(&dag.obsolete_commits)
    } else {
        commits
    };
    let commits = dag.query().sort(&commits)?;
    let commits = sort_commit_set(&repo, &dag, &commits)?;

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
            printable_styled_string(&glyphs, commit.friendly_describe(&glyphs)?)?,
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
