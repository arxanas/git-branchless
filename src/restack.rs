//! Handle "restacking" commits which were abandoned due to rewrites.
//!
//! The branchless workflow promotes checking out to arbitrary commits and
//! operating on them directly. However, if you e.g. amend a commit in-place, its
//! descendants will be abandoned.
//!
//! For example, suppose we have this graph:
//!
//! ```
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
//! ```
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
//! ```
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

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

use anyhow::Context;
use log::info;
use pyo3::prelude::*;

use crate::eventlog::{Event, EventLogDb, EventReplayer, PyEventReplayer};
use crate::graph::{
    make_graph, py_commit_graph_to_commit_graph, BranchOids, CommitGraph, HeadOid, MainBranchOid,
    PyCommitGraph, PyNode,
};
use crate::mergebase::MergeBaseDb;
use crate::python::{clone_conn, map_err_to_py_err, PyOidStr, TextIO};
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

fn restack_commits<Out: Write>(
    out: &mut Out,
    err: &mut Out,
    repo: &git2::Repository,
    git_executable: &GitExecutable,
    merge_base_db: &MergeBaseDb,
    event_log_db: &EventLogDb,
    preserve_timestamps: bool,
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
        let result = run_git(out, err, git_executable, &args)
            .with_context(|| format!("Running git at {:?} with args {:?}", git_executable, args))?;
        if result != 0 {
            writeln!(
                out,
                "branchless: resolve rebase, then run 'git restack' again"
            )?;
        }

        // Repeat until we reach a fixed point.
        return restack_commits(
            out,
            err,
            repo,
            git_executable,
            merge_base_db,
            event_log_db,
            preserve_timestamps,
        );
    }

    writeln!(out, "branchless: no more abandoned commits to restack")?;
    Ok(0)
}

fn restack_branches<Out: Write>(
    out: &mut Out,
    err: &mut Out,
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
/// * `preserve_timestamps`: Whether or not to use the original commit time for
/// rebased commits, rather than the current time.
///
/// Returns: Exit code (0 denotes successful exit).
pub fn restack<'out, Out: Write>(
    out: &'out mut Out,
    err: &'out mut Out,
    git_executable: GitExecutable,
    preserve_timestamps: bool,
) -> anyhow::Result<isize> {
    let repo = get_repo()?;
    let conn = get_db_conn(&repo)?;
    let merge_base_db = MergeBaseDb::new(clone_conn(&conn)?)?;
    let event_log_db = EventLogDb::new(clone_conn(&conn)?)?;
    let head_oid = get_head_oid(&repo)?;

    let result = restack_commits(
        out,
        err,
        &repo,
        &git_executable,
        &merge_base_db,
        &event_log_db,
        preserve_timestamps,
    )
    .with_context(|| "Restacking commits")?;
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
    )
    .with_context(|| "Restacking branches")?;
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

    // TODO: also display smartlog.
    Ok(result)
}

#[pyfunction]
fn py_find_abandoned_children(
    py: Python,
    graph: HashMap<PyOidStr, PyObject>,
    event_replayer: &PyEventReplayer,
    oid: PyOidStr,
) -> PyResult<Option<(PyOidStr, Vec<PyOidStr>)>> {
    let repo = get_repo();
    let repo = map_err_to_py_err(repo, "Getting repository")?;
    let graph: PyCommitGraph = graph
        .into_iter()
        .map(|(py_oid_str, py_node)| {
            let py_node: PyNode = py_node.as_ref(py).extract()?;
            Ok((py_oid_str, py_node))
        })
        .collect::<PyResult<PyCommitGraph>>()?;
    let graph = py_commit_graph_to_commit_graph(py, &repo, &graph);
    let graph = map_err_to_py_err(graph, "Converting PyCommitGraph to CommitGraph")?;
    let PyEventReplayer { event_replayer } = event_replayer;
    let PyOidStr(oid) = oid;

    let result = find_abandoned_children(&graph, event_replayer, oid).map(|(oid, child_oids)| {
        (
            PyOidStr(oid),
            child_oids.iter().copied().map(PyOidStr).collect(),
        )
    });
    Ok(result)
}

#[pyfunction]
fn py_restack(
    py: Python,
    out: PyObject,
    err: PyObject,
    git_executable: &str,
    preserve_timestamps: bool,
) -> PyResult<isize> {
    let mut out = TextIO::new(py, out);
    let mut err = TextIO::new(py, err);
    let git_executable = GitExecutable(Path::new(git_executable));
    let result = restack(&mut out, &mut err, git_executable, preserve_timestamps);
    let result = map_err_to_py_err(result, "Restack failed")?;
    Ok(result)
}

#[allow(missing_docs)]
pub fn register_python_symbols(module: &PyModule) -> PyResult<()> {
    module.add_function(pyo3::wrap_pyfunction!(py_find_abandoned_children, module)?)?;
    module.add_function(pyo3::wrap_pyfunction!(py_restack, module)?)?;
    Ok(())
}
