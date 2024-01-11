use std::fmt::Write;
use std::{collections::HashSet, time::SystemTime};

use itertools::Itertools;
use lib::core::effects::WithProgress;
use lib::git::{CategorizedReferenceName, MaybeZeroOid};
use lib::util::EyreExitOr;
use lib::{
    core::{
        effects::{Effects, OperationType},
        eventlog::{Event, EventLogDb, EventReplayer},
        formatting::Pluralize,
    },
    git::Repo,
};

pub fn repair(effects: &Effects, dry_run: bool) -> EyreExitOr<()> {
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();

    let broken_commits = {
        let (effects, progress) = effects.start_operation(OperationType::RepairCommits);
        let _effects = effects;
        let cursor_oids = event_replayer.get_cursor_oids(event_cursor);
        let mut result = HashSet::new();
        for oid in cursor_oids.into_iter().with_progress(progress) {
            if repo.find_commit(oid)?.is_none() {
                result.insert(oid);
            }
        }
        result
    };

    let broken_branches = {
        let (effects, progress) = effects.start_operation(OperationType::RepairBranches);
        let _effects = effects;
        let references_snapshot = event_replayer.get_references_snapshot(&repo, event_cursor)?;
        let branch_names = references_snapshot
            .branch_oid_to_names
            .into_iter()
            .flat_map(|(oid, reference_names)| {
                reference_names
                    .into_iter()
                    .map(move |reference_name| (oid, reference_name))
            })
            .collect_vec();
        let mut result = HashSet::new();
        for (oid, reference_name) in branch_names.into_iter().with_progress(progress) {
            if repo.find_reference(&reference_name)?.is_none() {
                result.insert((oid, reference_name));
            }
        }
        result
    };

    let now = SystemTime::now();
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();
    let event_tx_id = event_log_db.make_transaction_id(now, "repair")?;

    let num_broken_commits = broken_commits.len();
    let commit_events = broken_commits
        .iter()
        .map(|commit_oid| Event::ObsoleteEvent {
            timestamp,
            event_tx_id,
            commit_oid: *commit_oid,
        })
        .collect_vec();
    let num_broken_branches = broken_branches.len();
    let branch_events =
        broken_branches
            .iter()
            .map(|(old_oid, reference_name)| Event::RefUpdateEvent {
                timestamp,
                event_tx_id,
                ref_name: reference_name.to_owned(),
                old_oid: MaybeZeroOid::NonZero(*old_oid),
                new_oid: MaybeZeroOid::Zero,
                message: None,
            });

    if !dry_run {
        let events = commit_events.into_iter().chain(branch_events).collect_vec();
        event_log_db.add_events(events)?;
    }

    if num_broken_commits > 0 {
        writeln!(
            effects.get_output_stream(),
            "Found and repaired {}: {}",
            Pluralize {
                determiner: None,
                amount: num_broken_commits,
                unit: ("broken commit", "broken commits")
            },
            broken_commits.into_iter().sorted().join(", "),
        )?;
    }
    if num_broken_branches > 0 {
        writeln!(
            effects.get_output_stream(),
            "Found and repaired {}: {}",
            Pluralize {
                determiner: None,
                amount: num_broken_branches,
                unit: ("broken branch", "broken branches")
            },
            broken_branches
                .into_iter()
                .map(
                    |(_oid, reference_name)| CategorizedReferenceName::new(&reference_name)
                        .render_suffix()
                )
                .sorted()
                .join(", "),
        )?;
    }

    if dry_run {
        writeln!(
            effects.get_output_stream(),
            "(This was a dry-run; run with --no-dry-run to apply changes.)"
        )?;
    }

    Ok(Ok(()))
}
