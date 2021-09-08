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
use std::fmt::Write;
use std::time::SystemTime;

use tracing::{instrument, warn};

use crate::commands::smartlog::smartlog;
use crate::core::config::get_restack_preserve_timestamps;
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::graph::{make_graph, resolve_commits, ResolveCommitsResult};
use crate::core::rewrite::{
    execute_rebase_plan, find_abandoned_children, find_rewrite_target, move_branches,
    BuildRebasePlanOptions, ExecuteRebasePlanOptions, RebasePlanBuilder,
};
use crate::git::{Dag, GitRunInfo, NonZeroOid, Repo};
use crate::tui::Effects;

#[instrument(skip(commits))]
fn restack_commits(
    effects: &Effects,
    repo: &Repo,
    conn: &rusqlite::Connection,
    git_run_info: &GitRunInfo,
    event_log_db: &EventLogDb,
    commits: Option<impl IntoIterator<Item = NonZeroOid>>,
    build_options: &BuildRebasePlanOptions,
    execute_options: &ExecuteRebasePlanOptions,
) -> eyre::Result<isize> {
    let references_snapshot = repo.get_references_snapshot()?;
    let event_replayer = EventReplayer::from_event_log_db(effects, repo, event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let dag = Dag::open_and_sync(
        effects,
        repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;
    let graph = make_graph(effects, repo, &dag, &event_replayer, event_cursor, true)?;

    struct RebaseInfo {
        dest_oid: NonZeroOid,
        abandoned_child_oids: Vec<NonZeroOid>,
    }
    let commits: HashSet<NonZeroOid> = match commits {
        Some(commits) => commits.into_iter().collect(),
        None => graph.keys().copied().collect(),
    };
    let rebases: Vec<RebaseInfo> = {
        let mut result = Vec::new();
        for original_oid in commits {
            let abandoned_children =
                find_abandoned_children(&dag, &event_replayer, event_cursor, original_oid)?;
            if let Some((rewritten_oid, abandoned_child_oids)) = abandoned_children {
                result.push(RebaseInfo {
                    dest_oid: rewritten_oid,
                    abandoned_child_oids,
                });
            }
        }
        result
    };

    let rebase_plan = {
        let mut builder =
            RebasePlanBuilder::new(repo, &graph, &dag, references_snapshot.main_branch_oid);
        for RebaseInfo {
            dest_oid,
            abandoned_child_oids,
        } in rebases
        {
            for child_oid in abandoned_child_oids {
                builder.move_subtree(child_oid, dest_oid)?;
            }
        }
        builder.build(effects, build_options)?
    };

    match rebase_plan {
        Ok(None) => {
            writeln!(
                effects.get_output_stream(),
                "No abandoned commits to restack."
            )?;
            Ok(0)
        }
        Ok(Some(rebase_plan)) => {
            let exit_code =
                execute_rebase_plan(effects, git_run_info, repo, &rebase_plan, execute_options)?;
            match exit_code {
                0 => {
                    writeln!(effects.get_output_stream(), "Finished restacking commits.")?;
                }
                exit_code => {
                    writeln!(
                        effects.get_output_stream(),
                        "Error: Could not restack commits (exit code {}).",
                        exit_code
                    )?;
                    writeln!(
                        effects.get_output_stream(),
                        "You can resolve the error and try running `git restack` again."
                    )?;
                }
            }
            Ok(exit_code)
        }
        Err(err) => {
            err.describe(effects, repo)?;
            Ok(1)
        }
    }
}

#[instrument]
fn restack_branches(
    effects: &Effects,
    repo: &Repo,
    conn: &rusqlite::Connection,
    git_run_info: &GitRunInfo,
    event_log_db: &EventLogDb,
    options: &ExecuteRebasePlanOptions,
) -> eyre::Result<isize> {
    let references_snapshot = repo.get_references_snapshot()?;
    let event_replayer = EventReplayer::from_event_log_db(effects, repo, event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let dag = Dag::open_and_sync(
        effects,
        repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let graph = make_graph(effects, repo, &dag, &event_replayer, event_cursor, true)?;

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
            &event_replayer,
            event_replayer.make_default_cursor(),
            branch_target,
        ) {
            rewritten_oids.insert(branch_target, new_oid);
        };
    }

    if rewritten_oids.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "No abandoned branches to restack."
        )?;
    } else {
        move_branches(
            effects,
            git_run_info,
            repo,
            options.event_tx_id,
            &rewritten_oids,
        )?;
        writeln!(effects.get_output_stream(), "Finished restacking branches.")?;
    }
    Ok(0)
}

/// Restack all abandoned commits.
///
/// Returns an exit code (0 denotes successful exit).
#[instrument]
pub fn restack(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    commits: Vec<String>,
    force_in_memory: bool,
    force_on_disk: bool,
    dump_rebase_constraints: bool,
    dump_rebase_plan: bool,
) -> eyre::Result<isize> {
    let now = SystemTime::now();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "restack")?;
    let head_oid = repo.get_head_info()?.oid;

    let commits = match resolve_commits(&repo, commits)? {
        ResolveCommitsResult::Ok { commits } => commits,
        ResolveCommitsResult::CommitNotFound { commit } => {
            writeln!(effects.get_output_stream(), "Commit not found: {}", commit)?;
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
        detect_duplicate_commits_via_patch_id: true,
    };
    let execute_options = ExecuteRebasePlanOptions {
        now,
        event_tx_id,
        preserve_timestamps: get_restack_preserve_timestamps(&repo)?,
        force_in_memory,
        force_on_disk,
    };

    let result = restack_commits(
        effects,
        &repo,
        &conn,
        git_run_info,
        &event_log_db,
        commits,
        &build_options,
        &execute_options,
    )?;
    if result != 0 {
        return Ok(result);
    }

    let result = restack_branches(
        effects,
        &repo,
        &conn,
        git_run_info,
        &event_log_db,
        &execute_options,
    )?;
    if result != 0 {
        return Ok(result);
    }

    let result = match head_oid {
        Some(head_oid) => git_run_info.run(
            effects,
            Some(event_tx_id),
            &["checkout", &head_oid.to_string()],
        )?,
        None => result,
    };

    smartlog(effects)?;
    Ok(result)
}
