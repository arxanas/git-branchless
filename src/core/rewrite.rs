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
///
/// If a commit was rewritten into itself through some chain of events, then
/// returns `None`, rather than the same commit OID.
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

#[cfg(test)]
mod tests {
    use crate::core::eventlog::EventLogDb;
    use crate::core::graph::{make_graph, BranchOids, HeadOid, MainBranchOid};
    use crate::core::mergebase::MergeBaseDb;
    use crate::testing::{with_git, Git, GitRunOptions};
    use crate::util::{get_branch_oid_to_names, get_db_conn, get_head_oid, get_main_branch_oid};

    use super::*;

    fn find_rewrite_target_helper(git: &Git, oid: git2::Oid) -> anyhow::Result<Option<git2::Oid>> {
        let repo = git.get_repo()?;
        let conn = get_db_conn(&repo)?;
        let merge_base_db = MergeBaseDb::new(&conn)?;
        let event_log_db = EventLogDb::new(&conn)?;
        let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
        let event_cursor = event_replayer.make_default_cursor();
        let head_oid = get_head_oid(&repo)?;
        let main_branch_oid = get_main_branch_oid(&repo)?;
        let branch_oid_to_names = get_branch_oid_to_names(&repo)?;
        let graph = make_graph(
            &repo,
            &merge_base_db,
            &event_replayer,
            event_cursor,
            &HeadOid(head_oid),
            &MainBranchOid(main_branch_oid),
            &BranchOids(branch_oid_to_names.keys().copied().collect()),
            true,
        )?;

        let rewrite_target = find_rewrite_target(&graph, &event_replayer, event_cursor, oid);
        Ok(rewrite_target)
    }

    #[test]
    fn test_find_rewrite_target() -> anyhow::Result<()> {
        with_git(|git| {
            git.init_repo()?;
            let commit_time = 1;
            let old_oid = git.commit_file("test1", commit_time)?;

            {
                git.run(&["commit", "--amend", "-m", "test1 amended once"])?;
                let new_oid: git2::Oid = {
                    let (stdout, _stderr) = git.run(&["rev-parse", "HEAD"])?;
                    stdout.trim().parse()?
                };
                let rewrite_target = find_rewrite_target_helper(&git, old_oid)?;
                assert_eq!(rewrite_target, Some(new_oid));
            }

            {
                git.run(&["commit", "--amend", "-m", "test1 amended twice"])?;
                let new_oid: git2::Oid = {
                    let (stdout, _stderr) = git.run(&["rev-parse", "HEAD"])?;
                    stdout.trim().parse()?
                };
                let rewrite_target = find_rewrite_target_helper(&git, old_oid)?;
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
                let rewrite_target = find_rewrite_target_helper(&git, old_oid)?;
                assert_eq!(rewrite_target, None);
            }

            Ok(())
        })
    }
}
