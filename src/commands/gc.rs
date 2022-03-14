//! Deal with Git's garbage collection mechanism.
//!
//! Git treats a commit as unreachable if there are no references that point to
//! it or one of its descendants. However, the branchless workflow requires
//! keeping such commits reachable until the user has obsoleted them.
//!
//! This module is responsible for adding extra references to Git, so that Git's
//! garbage collection doesn't collect commits which branchless thinks are still
//! active.

use std::convert::TryInto;
use std::ffi::OsStr;
use std::fmt::Write;

use eyre::Context;
use tracing::instrument;

use crate::core::effects::Effects;
use crate::core::eventlog::{
    is_gc_ref, CommitActivityStatus, EventCursor, EventLogDb, EventReplayer,
};
use crate::core::formatting::Pluralize;
use crate::git::{NonZeroOid, Reference, Repo};

fn find_dangling_references<'repo>(
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
            CommitActivityStatus::Inactive | CommitActivityStatus::Obsolete => {
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
    if !repo.get_config_path().exists() {
        // Don't create garbage collection references for users who don't appear
        // to have run `git branchless init` in this repository, since they may
        // not want their tooling (such as `tig`) polluted by extra references.
        // See https://github.com/arxanas/git-branchless/issues/183
        return Ok(());
    }

    let ref_name = format!("refs/branchless/{}", commit_oid);
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
    let event_cursor = event_replayer.make_default_cursor();

    writeln!(
        effects.get_output_stream(),
        "branchless: collecting garbage"
    )?;
    let dangling_references = find_dangling_references(&repo, &event_replayer, event_cursor)?;
    let num_dangling_references = Pluralize {
        determiner: None,
        amount: dangling_references.len().try_into()?,
        unit: ("dangling reference", "dangling references"),
    }
    .to_string();
    for mut reference in dangling_references.into_iter() {
        reference.delete()?;
    }

    writeln!(
        effects.get_output_stream(),
        "branchless: {} deleted",
        num_dangling_references,
    )?;
    Ok(())
}
