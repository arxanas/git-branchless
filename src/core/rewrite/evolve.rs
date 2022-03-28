use std::convert::TryFrom;

use eden_dag::DagAlgorithm;
use itertools::Itertools;
use tracing::instrument;

use crate::core::dag::{CommitSet, Dag};
use crate::core::eventlog::{Event, EventCursor, EventReplayer};
use crate::git::{MaybeZeroOid, NonZeroOid};

/// For a rewritten commit, find the newest version of the commit.
///
/// For example, if we amend commit `abc` into commit `def1`, and then amend
/// `def1` into `def2`, then we can traverse the event log to find out that `def2`
/// is the newest version of `abc`.
///
/// If a commit was rewritten into itself through some chain of events, then
/// returns `None`, rather than the same commit OID.
#[instrument]
pub fn find_rewrite_target(
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    oid: NonZeroOid,
) -> Option<MaybeZeroOid> {
    let event = event_replayer.get_cursor_commit_latest_event(event_cursor, oid);
    let event = match event {
        Some(event) => event,
        None => return None,
    };
    match event {
        Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid: MaybeZeroOid::NonZero(old_commit_oid),
            new_commit_oid,
        } => {
            if *old_commit_oid == oid && *new_commit_oid != MaybeZeroOid::NonZero(oid) {
                match new_commit_oid {
                    MaybeZeroOid::Zero => Some(MaybeZeroOid::Zero),
                    MaybeZeroOid::NonZero(new_commit_oid) => {
                        let possible_newer_oid =
                            find_rewrite_target(event_replayer, event_cursor, *new_commit_oid);
                        match possible_newer_oid {
                            Some(newer_commit_oid) => Some(newer_commit_oid),
                            None => Some(MaybeZeroOid::NonZero(*new_commit_oid)),
                        }
                    }
                }
            } else {
                None
            }
        }

        Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid: MaybeZeroOid::Zero,
            new_commit_oid: _,
        }
        | Event::RefUpdateEvent { .. }
        | Event::CommitEvent { .. }
        | Event::ObsoleteEvent { .. }
        | Event::UnobsoleteEvent { .. } => None,
    }
}

/// Find commits which have been "abandoned" in the commit graph.
///
/// A commit is considered "abandoned" if it's not obsolete, but one of its
/// parents is.
#[instrument]
pub fn find_abandoned_children(
    dag: &Dag,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    oid: NonZeroOid,
) -> eyre::Result<Option<(NonZeroOid, Vec<NonZeroOid>)>> {
    let rewritten_oid = match find_rewrite_target(event_replayer, event_cursor, oid) {
        Some(MaybeZeroOid::NonZero(rewritten_oid)) => rewritten_oid,
        Some(MaybeZeroOid::Zero) => oid,
        None => return Ok(None),
    };

    let children = dag.query().children(CommitSet::from(oid))?;
    let children = children.intersection(&dag.observed_commits);
    let non_obsolete_children = children.difference(&dag.obsolete_commits);
    let non_obsolete_children_oids: Vec<NonZeroOid> = non_obsolete_children
        .iter()?
        .map(|x| -> eyre::Result<NonZeroOid> { NonZeroOid::try_from(x?) })
        .try_collect()?;

    Ok(Some((rewritten_oid, non_obsolete_children_oids)))
}

#[cfg(test)]
mod tests {
    use crate::core::effects::Effects;
    use crate::core::eventlog::EventLogDb;
    use crate::core::formatting::Glyphs;
    use crate::testing::{make_git, Git, GitRunOptions};

    use super::*;

    fn find_rewrite_target_helper(
        effects: &Effects,
        git: &Git,
        oid: NonZeroOid,
    ) -> eyre::Result<Option<MaybeZeroOid>> {
        let repo = git.get_repo()?;
        let conn = repo.get_db_conn()?;
        let event_log_db = EventLogDb::new(&conn)?;
        let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
        let event_cursor = event_replayer.make_default_cursor();

        let rewrite_target = find_rewrite_target(&event_replayer, event_cursor, oid);
        Ok(rewrite_target)
    }

    #[test]
    fn test_find_rewrite_target() -> eyre::Result<()> {
        let effects = Effects::new_suppress_for_test(Glyphs::text());
        let git = make_git()?;

        git.init_repo()?;
        let commit_time = 1;
        let old_oid = git.commit_file("test1", commit_time)?;

        {
            git.run(&["commit", "--amend", "-m", "test1 amended once"])?;
            let new_oid: MaybeZeroOid = {
                let (stdout, _stderr) = git.run(&["rev-parse", "HEAD"])?;
                stdout.trim().parse()?
            };
            let rewrite_target = find_rewrite_target_helper(&effects, &git, old_oid)?;
            assert_eq!(rewrite_target, Some(new_oid));
        }

        {
            git.run(&["commit", "--amend", "-m", "test1 amended twice"])?;
            let new_oid: MaybeZeroOid = {
                let (stdout, _stderr) = git.run(&["rev-parse", "HEAD"])?;
                stdout.trim().parse()?
            };
            let rewrite_target = find_rewrite_target_helper(&effects, &git, old_oid)?;
            assert_eq!(rewrite_target, Some(new_oid));
        }

        {
            git.run_with_options(
                &["commit", "--amend", "-m", "create test1.txt"],
                &GitRunOptions {
                    time: commit_time,
                    ..Default::default()
                },
            )?;
            let rewrite_target = find_rewrite_target_helper(&effects, &git, old_oid)?;
            assert_eq!(rewrite_target, None);
        }

        Ok(())
    }
}
