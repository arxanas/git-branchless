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

use std::collections::HashMap;
use std::time::SystemTime;

use anyhow::Context;
use fn_error_context::context;
use log::info;

use crate::commands::smartlog::smartlog;
use crate::core::config::get_restack_preserve_timestamps;
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::formatting::Glyphs;
use crate::core::graph::{make_graph, BranchOids, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::core::repo::Repo;
use crate::core::rewrite::{
    execute_rebase_plan, find_abandoned_children, find_rewrite_target, move_branches,
    ExecuteRebasePlanOptions, RebasePlanBuilder,
};
use crate::util::{get_branch_oid_to_names, get_db_conn, run_git, GitRunInfo};

#[context("Restacking commits")]
fn restack_commits(
    glyphs: &Glyphs,
    repo: &Repo,
    git_run_info: &GitRunInfo,
    merge_base_db: &MergeBaseDb,
    event_log_db: &EventLogDb,
    options: &ExecuteRebasePlanOptions,
) -> anyhow::Result<isize> {
    let event_replayer = EventReplayer::from_event_log_db(event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let head_oid = repo.get_head_oid()?;
    let main_branch_oid = repo.get_main_branch_oid()?;
    let branch_oid_to_names = get_branch_oid_to_names(repo)?;
    let graph = make_graph(
        repo,
        merge_base_db,
        &event_replayer,
        event_cursor,
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        true,
    )?;

    struct RebaseInfo {
        dest_oid: git2::Oid,
        abandoned_child_oids: Vec<git2::Oid>,
    }
    let rebases: Vec<RebaseInfo> = graph
        .keys()
        .copied()
        .filter_map(|original_oid| {
            find_abandoned_children(&graph, &event_replayer, event_cursor, original_oid).map(
                |(rewritten_oid, abandoned_child_oids)| RebaseInfo {
                    dest_oid: rewritten_oid,
                    abandoned_child_oids,
                },
            )
        })
        .collect();

    let rebase_plan = {
        let mut builder =
            RebasePlanBuilder::new(repo, &graph, merge_base_db, &MainBranchOid(main_branch_oid));
        for RebaseInfo {
            dest_oid,
            abandoned_child_oids,
        } in rebases
        {
            for child_oid in abandoned_child_oids {
                builder.move_subtree(child_oid, dest_oid)?;
            }
        }
        builder.build()
    };

    match rebase_plan {
        None => {
            println!("No abandoned commits to restack.");
            Ok(0)
        }
        Some(rebase_plan) => {
            let exit_code = execute_rebase_plan(glyphs, git_run_info, repo, &rebase_plan, options)?;
            println!("Finished restacking commits.");
            Ok(exit_code)
        }
    }
}

#[context("Restacking branches")]
fn restack_branches(
    repo: &Repo,
    git_run_info: &GitRunInfo,
    merge_base_db: &MergeBaseDb,
    event_log_db: &EventLogDb,
    options: &ExecuteRebasePlanOptions,
) -> anyhow::Result<isize> {
    let event_replayer = EventReplayer::from_event_log_db(event_log_db)?;
    let head_oid = repo.get_head_oid()?;
    let main_branch_oid = repo.get_main_branch_oid()?;
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

    let mut rewritten_oids = HashMap::new();
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

        if let Some(new_oid) = find_rewrite_target(
            &graph,
            &event_replayer,
            event_replayer.make_default_cursor(),
            branch_target,
        ) {
            rewritten_oids.insert(branch_target, new_oid);
        };
    }

    if rewritten_oids.is_empty() {
        println!("No abandoned branches to restack.");
    } else {
        move_branches(git_run_info, repo, options.event_tx_id, &rewritten_oids)?;
        println!("Finished restacking branches.");
    }
    Ok(0)
}

/// Restack all abandoned commits.
///
/// Returns an exit code (0 denotes successful exit).
#[context("Restacking commits and branches")]
pub fn restack(git_run_info: &GitRunInfo) -> anyhow::Result<isize> {
    let now = SystemTime::now();
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let conn = get_db_conn(&repo)?;
    let merge_base_db = MergeBaseDb::new(&conn)?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "restack")?;
    let head_oid = repo.get_head_oid()?;

    let options = ExecuteRebasePlanOptions {
        now,
        event_tx_id,
        preserve_timestamps: get_restack_preserve_timestamps(&repo)?,
        force_in_memory: false,
        // Use on-disk rebases only until `git move` is stabilized.
        force_on_disk: true,
    };

    let result = restack_commits(
        &glyphs,
        &repo,
        &git_run_info,
        &merge_base_db,
        &event_log_db,
        &options,
    )?;
    if result != 0 {
        return Ok(result);
    }

    let result = restack_branches(
        &repo,
        &git_run_info,
        &merge_base_db,
        &event_log_db,
        &options,
    )?;
    if result != 0 {
        return Ok(result);
    }

    let result = match head_oid {
        Some(head_oid) => run_git(
            &git_run_info,
            Some(event_tx_id),
            &["checkout", &head_oid.to_string()],
        )?,
        None => result,
    };

    smartlog()?;
    Ok(result)
}
