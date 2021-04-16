//! Utilities to deal with rewritten commits. See `Event::RewriteEvent` for
//! specifics on commit rewriting.

use std::collections::HashSet;

use super::eventlog::{Event, EventCursor, EventReplayer};
use super::graph::CommitGraph;

/// For a rewritten commit, find the newest version of the commit.
///
/// For example, if we amend commit `abc` into commit `def1`, and then amend
/// `def1` into `def2`, then we can traverse the event log to find out that `def2`
/// is the newest version of `abc`.
pub fn find_rewrite_target(
    graph: &CommitGraph,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    oid: git2::Oid,
) -> Option<git2::Oid> {
    let event = event_replayer.get_cursor_commit_latest_event(event_cursor, oid);
    let event = match event {
        Some(event) => event,
        None => return None,
    };
    match event {
        Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid,
            new_commit_oid,
        } => {
            if *old_commit_oid == oid && *new_commit_oid != oid {
                let possible_newer_oid =
                    find_rewrite_target(graph, event_replayer, event_cursor, *new_commit_oid);
                match possible_newer_oid {
                    Some(newer_commit_oid) => Some(newer_commit_oid),
                    None => Some(*new_commit_oid),
                }
            } else {
                None
            }
        }

        Event::RefUpdateEvent { .. }
        | Event::CommitEvent { .. }
        | Event::HideEvent { .. }
        | Event::UnhideEvent { .. } => None,
    }
}

/// Find commits which have been "abandoned" in the commit graph.
///
/// A commit is considered "abandoned" if it is visible, but one of its parents
/// is hidden.
pub fn find_abandoned_children(
    graph: &CommitGraph,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    oid: git2::Oid,
) -> Option<(git2::Oid, Vec<git2::Oid>)> {
    let rewritten_oid = find_rewrite_target(graph, event_replayer, event_cursor, oid)?;

    // Adjacent main branch commits are not linked in the commit graph, but if
    // the user rewrote a main branch commit, then we may need to restack
    // subsequent main branch commits. Find the real set of children commits so
    // that we can do this.
    let mut real_children_oids = graph[&oid].children.clone();
    let additional_children_oids: HashSet<git2::Oid> = graph
        .iter()
        .filter_map(|(possible_child_oid, possible_child_node)| {
            if real_children_oids.contains(possible_child_oid) {
                // Don't bother looking up the parents for commits we are
                // already including.
                None
            } else if possible_child_node
                .commit
                .parent_ids()
                .any(|parent_oid| parent_oid == oid)
            {
                Some(possible_child_oid)
            } else {
                None
            }
        })
        .copied()
        .collect();
    real_children_oids.extend(additional_children_oids);

    let visible_children_oids = real_children_oids
        .iter()
        .filter(|child_oid| graph[child_oid].is_visible)
        .copied()
        .collect();
    Some((rewritten_oid, visible_children_oids))
}
