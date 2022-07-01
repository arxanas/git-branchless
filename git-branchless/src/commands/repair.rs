use std::fmt::Write;
use std::{collections::HashSet, time::SystemTime};

use itertools::Itertools;
use lib::{
    core::{
        effects::{Effects, OperationType},
        eventlog::{Event, EventLogDb, EventReplayer},
        formatting::Pluralize,
    },
    git::Repo,
    util::ExitCode,
};

pub fn repair(effects: &Effects) -> eyre::Result<ExitCode> {
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();

    let broken_commits = {
        let (effects, progress) = effects.start_operation(OperationType::RepairCommits);
        let _effects = effects;
        let cursor_oids = event_replayer.get_cursor_oids(event_cursor);
        progress.notify_progress(0, cursor_oids.len());
        let mut result = HashSet::new();
        for oid in cursor_oids {
            if repo.find_commit(oid)?.is_none() {
                result.insert(oid);
            }
            progress.notify_progress_inc(1);
        }
        result
    };

    let now = SystemTime::now();
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();
    let event_tx_id = event_log_db.make_transaction_id(now, "repair")?;

    let num_broken_commits = broken_commits.len();
    let commit_events = broken_commits
        .into_iter()
        .map(|commit_oid| Event::ObsoleteEvent {
            timestamp,
            event_tx_id,
            commit_oid,
        })
        .collect_vec();

    let events = commit_events;
    event_log_db.add_events(events)?;
    writeln!(
        effects.get_output_stream(),
        "Found and repaired {}.",
        Pluralize {
            determiner: None,
            amount: num_broken_commits,
            unit: ("broken commit", "broken commits")
        }
    )?;

    Ok(ExitCode(0))
}
