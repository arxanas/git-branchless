//! Handle obsoleting commits when explicitly requested by the user (as opposed to
//! automatically as the result of a rewrite operation).

use std::fmt::Write;
use std::time::SystemTime;

use eden_dag::DagAlgorithm;
use tracing::instrument;

use crate::core::dag::{resolve_commits, sort_commit_set, CommitSet, Dag, ResolveCommitsResult};
use crate::core::effects::Effects;
use crate::core::eventlog::{CommitActivityStatus, Event};
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::formatting::{printable_styled_string, Glyphs};
use crate::core::node_descriptors::{render_node_descriptors, CommitOidDescriptor, NodeObject};
use crate::git::Repo;

/// Hide the hashes provided on the command-line.
#[instrument]
pub fn hide(effects: &Effects, hashes: Vec<String>, recursive: bool) -> eyre::Result<isize> {
    let now = SystemTime::now();
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let mut event_log_db = EventLogDb::new(&conn)?;
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
            return Ok(1);
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
    for commit in commits {
        writeln!(
            effects.get_output_stream(),
            "Hid commit: {}",
            printable_styled_string(&glyphs, commit.friendly_describe(&glyphs)?)?
        )?;
        if let CommitActivityStatus::Obsolete =
            event_replayer.get_cursor_commit_activity_status(cursor, commit.get_oid())
        {
            writeln!(
                effects.get_output_stream(),
                "(It was already hidden, so this operation had no effect.)"
            )?;
        }

        let commit_target_oid = render_node_descriptors(
            &glyphs,
            &NodeObject::Commit { commit },
            &mut [&mut CommitOidDescriptor::new(false)?],
        )?;
        writeln!(
            effects.get_output_stream(),
            "To unhide this commit, run: git unhide {}",
            printable_styled_string(&glyphs, commit_target_oid)?
        )?;
    }

    Ok(0)
}

/// Unhide the hashes provided on the command-line.
#[instrument]
pub fn unhide(effects: &Effects, hashes: Vec<String>, recursive: bool) -> eyre::Result<isize> {
    let now = SystemTime::now();
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let mut event_log_db = EventLogDb::new(&conn)?;
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
            return Ok(1);
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

        let commit_target_oid = render_node_descriptors(
            &glyphs,
            &NodeObject::Commit { commit },
            &mut [&mut CommitOidDescriptor::new(false)?],
        )?;
        writeln!(
            effects.get_output_stream(),
            "To hide this commit, run: git hide {}",
            printable_styled_string(&glyphs, commit_target_oid)?
        )?;
    }

    Ok(0)
}
