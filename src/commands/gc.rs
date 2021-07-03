//! Deal with Git's garbage collection mechanism.
//!
//! Git treats a commit as unreachable if there are no references that point to
//! it or one of its descendants. However, the branchless workflow oftentimes
//! involves keeping such commits reachable until the user has explicitly hidden
//! them.
//!
//! This module is responsible for adding extra references to Git, so that Git's
//! garbage collection doesn't collect commits which branchless thinks are still
//! visible.

use std::ffi::OsStr;

use anyhow::Context;
use fn_error_context::context;

use crate::core::eventlog::{is_gc_ref, EventLogDb, EventReplayer};
use crate::core::graph::{make_graph, BranchOids, CommitGraph, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::git::{Oid, Reference, Repo};

fn find_dangling_references<'repo>(
    repo: &'repo Repo,
    graph: &CommitGraph,
) -> anyhow::Result<Vec<Reference<'repo>>> {
    let mut result = Vec::new();
    for reference in repo.get_all_references()? {
        let reference_name = reference.get_name()?;

        // The graph only contains commits, so we don't need to handle the
        // case of the reference not peeling to a valid commit. (It might be
        // a reference to a different kind of object.)
        if let Some(commit) = reference.peel_to_commit()? {
            if is_gc_ref(&reference_name) && !graph.contains_key(&commit.get_oid()) {
                result.push(reference)
            }
        }
    }
    Ok(result)
}

/// Mark a commit as reachable.
///
/// Once marked as reachable, the commit won't be collected by Git's garbage
/// collection mechanism until first garbage-collected by branchless itself
/// (using the `gc` function).
///
/// Args:
/// * `repo`: The Git repository.
/// * `commit_oid`: The commit OID to mark as reachable.
#[context("Marking commit reachable: {:?}", commit_oid)]
pub fn mark_commit_reachable(repo: &Repo, commit_oid: Oid) -> anyhow::Result<()> {
    let ref_name = format!("refs/branchless/{}", commit_oid.to_string());
    anyhow::ensure!(
        Reference::is_valid_name(&ref_name),
        format!("Invalid ref name to mark commit as reachable: {}", ref_name)
    );
    repo.create_reference(
        OsStr::new(&ref_name),
        commit_oid,
        true,
        "branchless: marking commit as reachable",
    )
    .with_context(|| format!("Creating reference {}", ref_name))?;
    Ok(())
}

/// Run branchless's garbage collection.
///
/// Frees any references to commits which are no longer visible in the smartlog.
#[context("Running garbage-collection")]
pub fn gc() -> anyhow::Result<()> {
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let merge_base_db = MergeBaseDb::new(&conn)?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
    let head_oid = repo.get_head_info()?.oid;
    let main_branch_oid = repo.get_main_branch_oid()?;
    let branch_oid_to_names = repo.get_branch_oid_to_names()?;

    let graph = make_graph(
        &repo,
        &merge_base_db,
        &event_replayer,
        event_replayer.make_default_cursor(),
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        true,
    )?;

    println!("branchless: collecting garbage");
    let dangling_references = find_dangling_references(&repo, &graph)?;
    for mut reference in dangling_references.into_iter() {
        reference
            .delete()
            .with_context(|| format!("Deleting reference {:?}", reference.get_name()))?;
    }
    Ok(())
}
