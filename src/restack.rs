//! Handle "restacking" commits which were abandoned due to rewrites.
//!
//! The branchless workflow promotes checking out to arbitrary commits and
//! operating on them directly. However, if you e.g. amend a commit in-place, its
//! descendants will be abandoned.
//!
//! For example, suppose we have this graph:
//!
//! ```text
//! :
//! O abc000 master
//! |
//! @ abc001 Commit 1
//! |
//! o abc002 Commit 2
//! |
//! o abc003 Commit 3
//! ```
//!
//! And then we amend the current commit ("Commit 1"). The descendant commits
//! "Commit 2" and "Commit 3" will be abandoned:
//!
//! ```text
//! :
//! O abc000 master
//! |\\
//! | x abc001 Commit 1
//! | |
//! | o abc002 Commit 2
//! | |
//! | o abc003 Commit 3
//! |
//! o def001 Commit 1 amended
//! ```
//!
//! The "restack" operation finds abandoned commits and rebases them to where
//! they should belong, resulting in a commit graph like this (note that the
//! hidden commits would not ordinarily be displayed; we show them only for the
//! sake of example here):
//!
//! ```text
//! :
//! O abc000 master
//! |\\
//! | x abc001 Commit 1
//! | |
//! | x abc002 Commit 2
//! | |
//! | x abc003 Commit 3
//! |
//! o def001 Commit 1 amended
//! |
//! o def002 Commit 2
//! |
//! o def003 Commit 3
//! ```

use std::collections::HashSet;
use std::io::Write;

use anyhow::Context;
use fn_error_context::context;
use log::info;

use crate::config::get_restack_preserve_timestamps;
use crate::eventlog::{Event, EventLogDb, EventReplayer};
use crate::graph::{make_graph, BranchOids, CommitGraph, HeadOid, MainBranchOid};
use crate::mergebase::MergeBaseDb;
use crate::smartlog::smartlog;
use crate::util::{
    get_branch_oid_to_names, get_db_conn, get_head_oid, get_main_branch_oid, get_repo, run_git,
    GitExecutable,
};

fn find_rewrite_target(
    graph: &CommitGraph,
    event_replayer: &EventReplayer,
    oid: git2::Oid,
) -> Option<git2::Oid> {
    let event = event_replayer.get_cursor_commit_latest_event(oid);
    let event = match event {
        Some(event) => event,
        None => return None,
    };
    match event {
        Event::RewriteEvent {
            timestamp: _,
            old_commit_oid,
            new_commit_oid,
        } => {
            if *old_commit_oid == oid && *new_commit_oid != oid {
                let possible_newer_oid =
                    find_rewrite_target(graph, event_replayer, *new_commit_oid);
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
    oid: git2::Oid,
) -> Option<(git2::Oid, Vec<git2::Oid>)> {
    let rewritten_oid = find_rewrite_target(graph, event_replayer, oid)?;

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

#[context("Restacking commits")]
fn restack_commits(
    out: &mut impl Write,
    err: &mut impl Write,
    repo: &git2::Repository,
    git_executable: &GitExecutable,
    merge_base_db: &MergeBaseDb,
    event_log_db: &EventLogDb,
) -> anyhow::Result<isize> {
    let event_replayer = EventReplayer::from_event_log_db(event_log_db)?;
    let head_oid = get_head_oid(repo)?;
    let main_branch_oid = get_main_branch_oid(repo)?;
    let branch_oid_to_names = get_branch_oid_to_names(repo)?;
    let graph = make_graph(
        repo,
        merge_base_db,
        &event_replayer,
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        true,
    )?;
    let preserve_timestamps = get_restack_preserve_timestamps(&repo)?;

    for original_oid in graph.keys() {
        let (rewritten_oid, abandoned_child_oids) =
            match find_abandoned_children(&graph, &event_replayer, *original_oid) {
                Some(result) => result,
                None => continue,
            };

        // Pick an arbitrary abandoned child. We'll rewrite it and then repeat,
        // and next time, it won't be considered abandoned because it's been
        // rewritten.
        let abandoned_child_oid = match abandoned_child_oids.first() {
            Some(abandoned_child_oid) => abandoned_child_oid,
            None => continue,
        };

        let original_oid = original_oid.to_string();
        let abandoned_child_oid = abandoned_child_oid.to_string();
        let rewritten_oid = rewritten_oid.to_string();
        let args = {
            let mut args = vec![
                "rebase",
                &original_oid,
                &abandoned_child_oid,
                "--onto",
                &rewritten_oid,
            ];
            if preserve_timestamps {
                args.push("--committer-date-is-author-date");
            }
            args
        };
        let result = run_git(out, err, git_executable, &args)?;
        if result != 0 {
            writeln!(
                out,
                "branchless: resolve rebase, then run 'git restack' again"
            )?;
        }

        // Repeat until we reach a fixed point.
        return restack_commits(out, err, repo, git_executable, merge_base_db, event_log_db);
    }

    writeln!(out, "branchless: no more abandoned commits to restack")?;
    Ok(0)
}

#[context("Restacking branches")]
fn restack_branches(
    out: &mut impl Write,
    err: &mut impl Write,
    repo: &git2::Repository,
    git_executable: &GitExecutable,
    merge_base_db: &MergeBaseDb,
    event_log_db: &EventLogDb,
) -> anyhow::Result<isize> {
    let event_replayer = EventReplayer::from_event_log_db(event_log_db)?;
    let head_oid = get_head_oid(repo)?;
    let main_branch_oid = get_main_branch_oid(repo)?;
    let branch_oid_to_names = get_branch_oid_to_names(repo)?;
    let graph = make_graph(
        repo,
        merge_base_db,
        &event_replayer,
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        true,
    )?;

    for branch_info in repo
        .branches(Some(git2::BranchType::Local))
        .with_context(|| "Iterating over local branches")?
    {
        let (branch, _branch_type) = branch_info.with_context(|| "Getting branch info")?;
        let branch_target = match branch.get().target() {
            Some(branch_target) => branch_target,
            None => {
                info!(
                    "Branch {:?} was not a direct reference, could not resolve target",
                    branch.name()
                );
                continue;
            }
        };
        if !graph.contains_key(&branch_target) {
            continue;
        }

        let new_oid = match find_rewrite_target(&graph, &event_replayer, branch_target) {
            Some(new_oid) => new_oid.to_string(),
            None => continue,
        };
        let branch_name = match branch
            .name()
            .with_context(|| "Converting branch name to string")?
        {
            Some(branch_name) => branch_name,
            None => anyhow::bail!("Invalid UTF-8 branch name: {:?}", branch.name_bytes()?),
        };
        let args = ["branch", "-f", branch_name, &new_oid];
        let result = run_git(out, err, git_executable, &args)?;
        if result != 0 {
            return Ok(result);
        } else {
            return restack_branches(out, err, repo, git_executable, merge_base_db, event_log_db);
        }
    }

    writeln!(out, "branchless: no more abandoned branches to restack")?;
    Ok(0)
}

/// Restack all abandoned commits.
///
/// Args:
/// * `out`: The output stream to write to.
/// * `err`: The error stream to write to.
/// * `git_executable`: The path to the `git` executable on disk.
///
/// Returns: Exit code (0 denotes successful exit).
#[context("Restacking commits and branches")]
pub fn restack(
    out: &mut impl Write,
    err: &mut impl Write,
    git_executable: &GitExecutable,
) -> anyhow::Result<isize> {
    let repo = get_repo()?;
    let conn = get_db_conn(&repo)?;
    let merge_base_db = MergeBaseDb::new(&conn)?;
    let event_log_db = EventLogDb::new(&conn)?;
    let head_oid = get_head_oid(&repo)?;

    let result = restack_commits(
        out,
        err,
        &repo,
        &git_executable,
        &merge_base_db,
        &event_log_db,
    )?;
    if result != 0 {
        return Ok(result);
    }

    let result = restack_branches(
        out,
        err,
        &repo,
        &git_executable,
        &merge_base_db,
        &event_log_db,
    )?;
    if result != 0 {
        return Ok(result);
    }

    let result = match head_oid {
        Some(head_oid) => run_git(
            out,
            err,
            &git_executable,
            &["checkout", &head_oid.to_string()],
        )?,
        None => result,
    };

    smartlog(out)?;
    Ok(result)
}
