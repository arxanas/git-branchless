//! Handle hiding commits when explicitly requested by the user (as opposed to
//! automatically as the result of a rewrite operation).

use std::collections::HashSet;
use std::io::Write;
use std::time::SystemTime;

use fn_error_context::context;
use git2::ErrorCode;
use pyo3::prelude::*;

use crate::eventlog::{CommitVisibility, Event};
use crate::eventlog::{EventLogDb, EventReplayer};

use crate::graph::{make_graph, BranchOids, CommitGraph, HeadOid, MainBranchOid, Node};
use crate::mergebase::MergeBaseDb;
use crate::metadata::{
    render_commit_metadata, CommitMessageProvider, CommitMetadataProvider, CommitOidProvider,
};
use crate::python::{clone_conn, map_err_to_py_err, TextIO};
use crate::util::{
    get_branch_oid_to_names, get_db_conn, get_head_oid, get_main_branch_oid, get_repo,
};

enum ProcessHashesResult<'repo> {
    Ok { commits: Vec<git2::Commit<'repo>> },
    CommitNotFound { hash: String },
}

#[context("Processing hashes")]
fn process_hashes(
    repo: &git2::Repository,
    hashes: Vec<String>,
) -> anyhow::Result<ProcessHashesResult> {
    let mut commits = Vec::new();
    for hash in hashes {
        let commit = match repo.revparse_single(&hash) {
            Ok(commit) => match commit.into_commit() {
                Ok(commit) => commit,
                Err(_) => return Ok(ProcessHashesResult::CommitNotFound { hash }),
            },
            Err(err) if err.code() == ErrorCode::NotFound => {
                return Ok(ProcessHashesResult::CommitNotFound { hash })
            }
            Err(err) => return Err(err.into()),
        };
        commits.push(commit)
    }
    Ok(ProcessHashesResult::Ok { commits })
}

fn recurse_on_commits_helper<
    'repo,
    'graph,
    Condition: Fn(&'graph Node<'repo>) -> bool,
    Callback: FnMut(&'graph Node<'repo>),
>(
    graph: &'graph CommitGraph<'repo>,
    condition: &Condition,
    commit: &git2::Commit<'repo>,
    callback: &mut Callback,
) {
    let node = &graph[&commit.id()];
    if condition(node) {
        callback(node);
    };

    for child_oid in node.children.iter() {
        let child_commit = &graph[&child_oid].commit;
        recurse_on_commits_helper(graph, condition, child_commit, callback)
    }
}

fn recurse_on_commits<'repo, F: Fn(&Node) -> bool>(
    repo: &'repo git2::Repository,
    merge_base_db: &MergeBaseDb,
    event_replayer: &EventReplayer,
    commits: Vec<git2::Commit<'repo>>,
    condition: F,
) -> anyhow::Result<Vec<git2::Commit<'repo>>> {
    let head_oid = get_head_oid(repo)?;
    let main_branch_oid = get_main_branch_oid(repo)?;
    let branch_oid_to_names = get_branch_oid_to_names(repo)?;
    let graph = make_graph(
        repo,
        merge_base_db,
        event_replayer,
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        false,
    )?;

    // Maintain ordering, since it's likely to be meaningful.
    let mut result: Vec<git2::Commit<'repo>> = Vec::new();
    let mut seen_oids = HashSet::new();
    for commit in commits {
        recurse_on_commits_helper(&graph, &condition, &commit, &mut |child_node| {
            let child_commit = &child_node.commit;
            if !seen_oids.contains(&child_commit.id()) {
                seen_oids.insert(child_commit.id());
                result.push(child_commit.clone());
            }
        });
    }
    Ok(result)
}

/// Hide the hashes provided on the command-line.
///
/// Args:
/// * `out`: The output stream to write to.
/// * `hashes`: A list of commit hashes to hide. Revs will be resolved (you can
///   provide an abbreviated commit hash or ref name).
/// * `recursive: If `true`, will recursively hide all children of the provided
///   commits as well.
///
/// Returns: exit code (0 denotes successful exit).
pub fn hide<Out: Write>(
    out: &mut Out,
    hashes: Vec<String>,
    recursive: bool,
) -> anyhow::Result<isize> {
    let timestamp = SystemTime::now();
    let repo = get_repo()?;
    let conn = get_db_conn(&repo)?;
    let mut event_log_db = EventLogDb::new(clone_conn(&conn)?)?;
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
    let merge_base_db = MergeBaseDb::new(clone_conn(&conn)?)?;

    let commits = process_hashes(&repo, hashes)?;
    let commits = match commits {
        ProcessHashesResult::Ok { commits } => commits,
        ProcessHashesResult::CommitNotFound { hash } => {
            writeln!(out, "Commit not found: {}", hash)?;
            return Ok(1);
        }
    };
    let commits = if recursive {
        recurse_on_commits(&repo, &merge_base_db, &event_replayer, commits, |node| {
            node.is_visible
        })?
    } else {
        commits
    };

    let timestamp = timestamp
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs_f64();
    let events = commits
        .iter()
        .map(|commit| Event::HideEvent {
            timestamp,
            commit_oid: commit.id(),
        })
        .collect();
    event_log_db.add_events(events)?;

    for commit in commits {
        let hidden_commit_text = {
            let commit_oid_provider = CommitOidProvider::new(true)?;
            let commit_message_provider = CommitMessageProvider::new()?;
            let metadata_providers: Vec<&dyn CommitMetadataProvider> =
                vec![&commit_oid_provider, &commit_message_provider];
            render_commit_metadata(metadata_providers.into_iter(), &commit)?
        };
        writeln!(out, "Hid commit: {}", hidden_commit_text)?;
        if let Some(CommitVisibility::Hidden) =
            event_replayer.get_cursor_commit_visibility(commit.id())
        {
            writeln!(
                out,
                "(It was already hidden, so this operation had no effect.)"
            )?;
        }

        let commit_target_oid = {
            let commit_oid_provider = CommitOidProvider::new(false)?;
            let metadata_providers: Vec<&dyn CommitMetadataProvider> = vec![&commit_oid_provider];
            render_commit_metadata(metadata_providers.into_iter(), &commit)?
        };
        writeln!(
            out,
            "To unhide this commit, run: git unhide {}",
            commit_target_oid
        )?;
    }

    Ok(0)
}

/// Unhide the hashes provided on the command-line.
///
/// Args:
/// * `out`: The output stream to write to.
/// * `hashes`: A list of commit hashes to unhide. Revs will be resolved (you can
/// provide an abbreviated commit hash or ref name).
/// * `recursive: If `true`, will recursively unhide all children of the provided
///   commits as well.
///
/// Returns: exit code (0 denotes successful exit).
pub fn unhide<Out: Write>(
    out: &mut Out,
    hashes: Vec<String>,
    recursive: bool,
) -> anyhow::Result<isize> {
    let timestamp = SystemTime::now();
    let repo = get_repo()?;
    let conn = get_db_conn(&repo)?;
    let mut event_log_db = EventLogDb::new(clone_conn(&conn)?)?;
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
    let merge_base_db = MergeBaseDb::new(clone_conn(&conn)?)?;

    let commits = process_hashes(&repo, hashes)?;
    let commits = match commits {
        ProcessHashesResult::Ok { commits } => commits,
        ProcessHashesResult::CommitNotFound { hash } => {
            writeln!(out, "Commit not found: {}", hash)?;
            return Ok(1);
        }
    };
    let commits = if recursive {
        recurse_on_commits(&repo, &merge_base_db, &event_replayer, commits, |node| {
            !node.is_visible
        })?
    } else {
        commits
    };

    let timestamp = timestamp
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs_f64();
    let events = commits
        .iter()
        .map(|commit| Event::UnhideEvent {
            timestamp,
            commit_oid: commit.id(),
        })
        .collect();
    event_log_db.add_events(events)?;

    for commit in commits {
        let unhidden_commit_text = {
            let commit_oid_provider = CommitOidProvider::new(true)?;
            let commit_message_provider = CommitMessageProvider::new()?;
            let metadata_providers: Vec<&dyn CommitMetadataProvider> =
                vec![&commit_oid_provider, &commit_message_provider];
            render_commit_metadata(metadata_providers.into_iter(), &commit)?
        };
        writeln!(out, "Unhid commit: {}", unhidden_commit_text)?;
        if let Some(CommitVisibility::Visible) =
            event_replayer.get_cursor_commit_visibility(commit.id())
        {
            writeln!(out, "(It was not hidden, so this operation had no effect.)")?;
        }

        let commit_target_oid = {
            let commit_oid_provider = CommitOidProvider::new(false)?;
            let metadata_providers: Vec<&dyn CommitMetadataProvider> = vec![&commit_oid_provider];
            render_commit_metadata(metadata_providers.into_iter(), &commit)?
        };
        writeln!(
            out,
            "To hide this commit, run: git hide {}",
            commit_target_oid
        )?;
    }

    Ok(0)
}

#[pyfunction]
fn py_hide(py: Python, out: PyObject, hashes: Vec<String>, recursive: bool) -> PyResult<isize> {
    let mut out = TextIO::new(py, out);
    let result = hide(&mut out, hashes, recursive);
    let result = map_err_to_py_err(result, "Could not hide commits")?;
    Ok(result)
}

#[pyfunction]
fn py_unhide(py: Python, out: PyObject, hashes: Vec<String>, recursive: bool) -> PyResult<isize> {
    let mut out = TextIO::new(py, out);
    let result = unhide(&mut out, hashes, recursive);
    let result = map_err_to_py_err(result, "Could not hide commits")?;
    Ok(result)
}

#[allow(missing_docs)]
pub fn register_python_symbols(module: &PyModule) -> PyResult<()> {
    module.add_function(pyo3::wrap_pyfunction!(py_hide, module)?)?;
    module.add_function(pyo3::wrap_pyfunction!(py_unhide, module)?)?;
    Ok(())
}
