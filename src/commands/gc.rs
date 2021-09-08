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

use std::borrow::Borrow;
use std::ffi::OsStr;
use std::fmt::Write;

use eyre::Context;
use tracing::instrument;

use crate::core::eventlog::{is_gc_ref, EventLogDb, EventReplayer};
use crate::core::graph::{make_graph, BranchOids, CommitGraph, HeadOid, MainBranchOid};
use crate::core::mergebase::make_merge_base_db;
use crate::git::{NonZeroOid, Reference, Repo};
use crate::tui::Effects;

fn find_dangling_references<'repo>(
    repo: &'repo Repo,
    graph: &CommitGraph,
) -> eyre::Result<Vec<Reference<'repo>>> {
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
#[instrument]
pub fn mark_commit_reachable(repo: &Repo, commit_oid: NonZeroOid) -> eyre::Result<()> {
    let ref_name = format!("refs/branchless/{}", commit_oid.to_string());
    eyre::ensure!(
        Reference::is_valid_name(&ref_name),
        format!("Invalid ref name to mark commit as reachable: {}", ref_name)
    );
    repo.create_reference(
        OsStr::new(&ref_name),
        commit_oid,
        true,
        "branchless: marking commit as reachable",
    )
    .wrap_err("Creating reference")?;
    Ok(())
}

/// Run branchless's garbage collection.
///
/// Frees any references to commits which are no longer visible in the smartlog.
#[instrument]
pub fn gc(effects: &Effects) -> eyre::Result<()> {
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let merge_base_db = make_merge_base_db(effects, &repo, &conn, &event_replayer)?;
    let head_oid = repo.get_head_info()?.oid;
    let main_branch_oid = repo.get_main_branch_oid()?;
    let branch_oid_to_names = repo.get_branch_oid_to_names()?;

    let graph = make_graph(
        effects,
        &repo,
        merge_base_db.borrow(),
        &event_replayer,
        event_replayer.make_default_cursor(),
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        true,
    )?;

    writeln!(
        effects.get_output_stream(),
        "branchless: collecting garbage"
    )?;
    let dangling_references = find_dangling_references(&repo, &graph)?;
    for mut reference in dangling_references.into_iter() {
        reference.delete()?;
    }
    Ok(())
}
