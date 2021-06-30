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

use anyhow::Context;
use fn_error_context::context;

use crate::core::eventlog::{is_gc_ref, EventLogDb, EventReplayer};
use crate::core::graph::{make_graph, BranchOids, CommitGraph, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::core::repo::Repo;
use crate::util::{get_branch_oid_to_names, get_db_conn, get_main_branch_oid};

fn find_dangling_references<'repo>(
    repo: &'repo git2::Repository,
    graph: &CommitGraph,
) -> anyhow::Result<Vec<git2::Reference<'repo>>> {
    let references = repo
        .references()
        .with_context(|| "Getting repo references")?;

    let mut result = Vec::new();
    for reference in references {
        let reference = reference.with_context(|| "Reading reference info")?;
        let reference_name = match reference.name() {
            Some(name) => name.to_owned(),
            None => continue,
        };
        let resolved_reference = reference
            .resolve()
            .with_context(|| format!("Resolving reference: {}", reference_name))?;

        // The graph only contains commits, so we don't need to handle the
        // case of the reference not peeling to a valid commit. (It might be
        // a reference to a different kind of object.)
        if let Ok(commit) = resolved_reference.peel_to_commit() {
            if is_gc_ref(&reference_name) && !graph.contains_key(&commit.id()) {
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
pub fn mark_commit_reachable(repo: &git2::Repository, commit_oid: git2::Oid) -> anyhow::Result<()> {
    let ref_name = format!("refs/branchless/{}", commit_oid.to_string());
    anyhow::ensure!(
        git2::Reference::is_valid_name(&ref_name),
        format!("Invalid ref name to mark commit as reachable: {}", ref_name)
    );
    repo.reference(
        &ref_name,
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
    let conn = get_db_conn(&repo)?;
    let merge_base_db = MergeBaseDb::new(&conn)?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
    let head_oid = repo.get_head_oid()?;
    let main_branch_oid = get_main_branch_oid(&repo)?;
    let branch_oid_to_names = get_branch_oid_to_names(&repo)?;

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
            .with_context(|| format!("Deleting reference {:?}", reference.name()))?;
    }
    Ok(())
}
