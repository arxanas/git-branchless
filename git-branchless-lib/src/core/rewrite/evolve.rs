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
        | Event::UnobsoleteEvent { .. }
        | Event::WorkingCopySnapshot { .. } => None,
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
    let children = dag.query_children(CommitSet::from(oid))?;
    let children = dag.filter_visible_commits(children)?;
    let non_obsolete_children = children.difference(&dag.query_obsolete_commits());
    let non_obsolete_children_oids = dag.commit_set_to_vec(&non_obsolete_children)?;

    Ok(Some((rewritten_oid, non_obsolete_children_oids)))
}
