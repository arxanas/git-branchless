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

use std::time::SystemTime;

use anyhow::Context;
use fn_error_context::context;
use log::info;

use crate::commands::smartlog::smartlog;
use crate::core::config::get_restack_preserve_timestamps;
use crate::core::eventlog::{EventLogDb, EventReplayer, EventTransactionId};
use crate::core::graph::{make_graph, BranchOids, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::core::rewrite::{find_abandoned_children, find_rewrite_target};
use crate::util::{
    get_branch_oid_to_names, get_db_conn, get_head_oid, get_main_branch_oid, get_repo, run_git,
    GitExecutable,
};

#[context("Restacking commits")]
fn restack_commits(
    repo: &git2::Repository,
    git_executable: &GitExecutable,
    merge_base_db: &MergeBaseDb,
    event_log_db: &EventLogDb,
    event_tx_id: EventTransactionId,
) -> anyhow::Result<isize> {
    let event_replayer = EventReplayer::from_event_log_db(event_log_db)?;
    let head_oid = get_head_oid(repo)?;
    let main_branch_oid = get_main_branch_oid(repo)?;
    let branch_oid_to_names = get_branch_oid_to_names(repo)?;
    let graph = make_graph(
        repo,
        merge_base_db,
        &event_replayer,
        event_replayer.make_default_cursor(),
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        true,
    )?;
    let preserve_timestamps = get_restack_preserve_timestamps(&repo)?;

    for original_oid in graph.keys() {
        let (rewritten_oid, abandoned_child_oids) = match find_abandoned_children(
            &graph,
            &event_replayer,
            event_replayer.make_default_cursor(),
            *original_oid,
        ) {
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
        let result = run_git(git_executable, Some(event_tx_id), &args)?;
        if result != 0 {
            println!("branchless: resolve rebase, then run 'git restack' again");
            return Ok(result);
        }

        // Repeat until we reach a fixed point.
        return restack_commits(
            repo,
            git_executable,
            merge_base_db,
            event_log_db,
            event_tx_id,
        );
    }

    println!("branchless: no more abandoned commits to restack");
    Ok(0)
}

#[context("Restacking branches")]
fn restack_branches(
    repo: &git2::Repository,
    git_executable: &GitExecutable,
    merge_base_db: &MergeBaseDb,
    event_log_db: &EventLogDb,
    event_tx_id: EventTransactionId,
) -> anyhow::Result<isize> {
    let event_replayer = EventReplayer::from_event_log_db(event_log_db)?;
    let head_oid = get_head_oid(repo)?;
    let main_branch_oid = get_main_branch_oid(repo)?;
    let branch_oid_to_names = get_branch_oid_to_names(repo)?;
    let graph = make_graph(
        repo,
        merge_base_db,
        &event_replayer,
        event_replayer.make_default_cursor(),
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

        let new_oid = match find_rewrite_target(
            &graph,
            &event_replayer,
            event_replayer.make_default_cursor(),
            branch_target,
        ) {
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
        let result = run_git(git_executable, Some(event_tx_id), &args)?;
        if result != 0 {
            return Ok(result);
        } else {
            return restack_branches(
                repo,
                git_executable,
                merge_base_db,
                event_log_db,
                event_tx_id,
            );
        }
    }

    println!("branchless: no more abandoned branches to restack");
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
pub fn restack(git_executable: &GitExecutable) -> anyhow::Result<isize> {
    let repo = get_repo()?;
    let conn = get_db_conn(&repo)?;
    let merge_base_db = MergeBaseDb::new(&conn)?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(SystemTime::now(), "restack")?;
    let head_oid = get_head_oid(&repo)?;

    let result = restack_commits(
        &repo,
        &git_executable,
        &merge_base_db,
        &event_log_db,
        event_tx_id,
    )?;
    if result != 0 {
        return Ok(result);
    }

    let result = restack_branches(
        &repo,
        &git_executable,
        &merge_base_db,
        &event_log_db,
        event_tx_id,
    )?;
    if result != 0 {
        return Ok(result);
    }

    let result = match head_oid {
        Some(head_oid) => run_git(
            &git_executable,
            Some(event_tx_id),
            &["checkout", &head_oid.to_string()],
        )?,
        None => result,
    };

    smartlog()?;
    Ok(result)
}
