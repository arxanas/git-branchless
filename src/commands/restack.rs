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

use std::collections::{HashMap, HashSet};
use std::io::stdout;
use std::time::SystemTime;

use tracing::{instrument, warn};

use crate::commands::smartlog::smartlog;
use crate::core::config::get_restack_preserve_timestamps;
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::formatting::Glyphs;
use crate::core::graph::{
    make_graph, resolve_commits, BranchOids, HeadOid, MainBranchOid, ResolveCommitsResult,
};
use crate::core::mergebase::MergeBaseDb;
use crate::core::rewrite::{
    execute_rebase_plan, find_abandoned_children, find_rewrite_target, move_branches,
    BuildRebasePlanOptions, ExecuteRebasePlanOptions, RebasePlanBuilder,
};
use crate::git::{GitRunInfo, NonZeroOid, Repo};

#[instrument(skip(commits))]
fn restack_commits(
    glyphs: &Glyphs,
    repo: &Repo,
    git_run_info: &GitRunInfo,
    merge_base_db: &MergeBaseDb,
    event_log_db: &EventLogDb,
    commits: Option<impl IntoIterator<Item = NonZeroOid>>,
    build_options: &BuildRebasePlanOptions,
    execute_options: &ExecuteRebasePlanOptions,
) -> eyre::Result<isize> {
    let event_replayer = EventReplayer::from_event_log_db(repo, event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let head_oid = repo.get_head_info()?.oid;
    let main_branch_oid = repo.get_main_branch_oid()?;
    let branch_oid_to_names = repo.get_branch_oid_to_names()?;
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
        dest_oid: NonZeroOid,
        abandoned_child_oids: Vec<NonZeroOid>,
    }
    let commits: HashSet<NonZeroOid> = match commits {
        Some(commits) => commits.into_iter().collect(),
        None => graph.keys().copied().collect(),
    };
    let rebases: Vec<RebaseInfo> = commits
        .into_iter()
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
        builder.build(build_options)?
    };

    match rebase_plan {
        Ok(None) => {
            println!("No abandoned commits to restack.");
            Ok(0)
        }
        Ok(Some(rebase_plan)) => {
            let exit_code =
                execute_rebase_plan(glyphs, git_run_info, repo, &rebase_plan, execute_options)?;
            println!("Finished restacking commits.");
            Ok(exit_code)
        }
        Err(err) => {
            err.describe(&mut stdout(), glyphs, repo)?;
            Ok(1)
        }
    }
}

#[instrument]
fn restack_branches(
    repo: &Repo,
    git_run_info: &GitRunInfo,
    merge_base_db: &MergeBaseDb,
    event_log_db: &EventLogDb,
    options: &ExecuteRebasePlanOptions,
) -> eyre::Result<isize> {
    let event_replayer = EventReplayer::from_event_log_db(repo, event_log_db)?;
    let head_oid = repo.get_head_info()?.oid;
    let main_branch_oid = repo.get_main_branch_oid()?;
    let branch_oid_to_names = repo.get_branch_oid_to_names()?;
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
    for branch in repo.get_all_local_branches()? {
        let branch_target = match branch.get_oid()? {
            Some(branch_target) => branch_target,
            None => {
                warn!(
                    branch_name = ?branch.into_reference().get_name(),
                    "Branch was not a direct reference, could not resolve target"
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
#[instrument]
pub fn restack(
    git_run_info: &GitRunInfo,
    commits: Vec<String>,
    dump_rebase_constraints: bool,
    dump_rebase_plan: bool,
) -> eyre::Result<isize> {
    let now = SystemTime::now();
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let merge_base_db = MergeBaseDb::new(&conn)?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "restack")?;
    let head_oid = repo.get_head_info()?.oid;

    let commits = match resolve_commits(&repo, commits)? {
        ResolveCommitsResult::Ok { commits } => commits,
        ResolveCommitsResult::CommitNotFound { commit } => {
            println!("Commit not found: {}", commit);
            return Ok(1);
        }
    };
    let commits: Option<HashSet<NonZeroOid>> = if commits.is_empty() {
        None
    } else {
        Some(commits.into_iter().map(|commit| commit.get_oid()).collect())
    };

    let build_options = BuildRebasePlanOptions {
        dump_rebase_constraints,
        dump_rebase_plan,
    };
    let execute_options = ExecuteRebasePlanOptions {
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
        commits,
        &build_options,
        &execute_options,
    )?;
    if result != 0 {
        return Ok(result);
    }

    let result = restack_branches(
        &repo,
        &git_run_info,
        &merge_base_db,
        &event_log_db,
        &execute_options,
    )?;
    if result != 0 {
        return Ok(result);
    }

    let result = match head_oid {
        Some(head_oid) => {
            git_run_info.run(Some(event_tx_id), &["checkout", &head_oid.to_string()])?
        }
        None => result,
    };

    smartlog()?;
    Ok(result)
}
