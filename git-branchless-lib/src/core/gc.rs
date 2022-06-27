//! Deal with Git's garbage collection mechanism.
//!
//! Git treats a commit as unreachable if there are no references that point to
//! it or one of its descendants. However, the branchless workflow requires
//! keeping such commits reachable until the user has obsoleted them.
//!
//! This module is responsible for adding extra references to Git, so that Git's
//! garbage collection doesn't collect commits which branchless thinks are still
//! active.

use std::ffi::OsStr;

use eyre::Context;
use tracing::instrument;

use crate::git::{NonZeroOid, Reference, Repo};

use super::eventlog::{is_gc_ref, CommitActivityStatus, EventCursor, EventReplayer};

/// Find references under `refs/branchless/` which point to commits which are no
/// longer active. These are safe to remove.
pub fn find_dangling_references<'repo>(
    repo: &'repo Repo,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
) -> eyre::Result<Vec<Reference<'repo>>> {
    let mut result = Vec::new();
    for reference in repo.get_all_references()? {
        let reference_name = reference.get_name()?;
        if !is_gc_ref(&reference_name) {
            continue;
        }

        // The graph only contains commits, so we don't need to handle the
        // case of the reference not peeling to a valid commit. (It might be
        // a reference to a different kind of object.)
        let commit = match reference.peel_to_commit()? {
            Some(commit) => commit,
            None => continue,
        };

        match event_replayer.get_cursor_commit_activity_status(event_cursor, commit.get_oid()) {
            CommitActivityStatus::Active => {
                // Do nothing.
            }
            CommitActivityStatus::Inactive => {
                // This commit hasn't been observed, but it's possible that the user expected it
                // to remain. Do nothing. See https://github.com/arxanas/git-branchless/issues/412.
            }
            CommitActivityStatus::Obsolete => {
                // This commit was explicitly hidden by some operation.
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
/// If the commit does not exist (such as if it was already garbage-collected), then this is a no-op.
///
/// Args:
/// * `repo`: The Git repository.
/// * `commit_oid`: The commit OID to mark as reachable.
#[instrument]
pub fn mark_commit_reachable(repo: &Repo, commit_oid: NonZeroOid) -> eyre::Result<()> {
    let ref_name = format!("refs/branchless/{}", commit_oid);
    eyre::ensure!(
        Reference::is_valid_name(&ref_name),
        format!("Invalid ref name to mark commit as reachable: {}", ref_name)
    );

    // NB: checking for the commit first with `find_commit` is racy, as the `create_reference` call
    // could still fail if the commit is deleted by then, but it's too hard to propagate whether the
    // commit was not found from `create_reference`.
    if repo.find_commit(commit_oid)?.is_some() {
        repo.create_reference(
            OsStr::new(&ref_name),
            commit_oid,
            true,
            "branchless: marking commit as reachable",
        )
        .wrap_err("Creating reference")?;
    }

    Ok(())
}
