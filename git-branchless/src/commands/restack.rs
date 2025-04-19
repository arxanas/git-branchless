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

use lib::core::check_out::CheckOutCommitOptions;
use lib::core::repo_ext::RepoExt;
use lib::try_exit_code;
use lib::util::{ExitCode, EyreExitOr};
use rayon::{ThreadPool, ThreadPoolBuilder};
use tracing::{instrument, warn};

use git_branchless_opts::{MoveOptions, ResolveRevsetOptions, Revset};
use git_branchless_revset::resolve_commits;
use git_branchless_smartlog::smartlog;
use lib::core::config::get_restack_preserve_timestamps;
use lib::core::dag::{union_all, CommitSet, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{EventCursor, EventLogDb, EventReplayer};
use lib::core::rewrite::{
    execute_rebase_plan, find_abandoned_children, find_rewrite_target, move_branches,
    BuildRebasePlanOptions, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    MergeConflictRemediation, RebasePlanBuilder, RebasePlanPermissions, RepoPool, RepoResource,
};
use lib::git::{GitRunInfo, NonZeroOid, Repo};

#[instrument(skip(commits))]
fn restack_commits(
    effects: &Effects,
    thread_pool: &ThreadPool,
    repo_pool: &RepoPool,
    dag: &Dag,
    event_replayer: &EventReplayer,
    event_log_db: &EventLogDb,
    event_cursor: EventCursor,
    git_run_info: &GitRunInfo,
    commits: Option<impl IntoIterator<Item = NonZeroOid>>,
    build_options: BuildRebasePlanOptions,
    execute_options: &ExecuteRebasePlanOptions,
    merge_conflict_remediation: MergeConflictRemediation,
) -> EyreExitOr<()> {
    let repo = repo_pool.try_create()?;
    let commit_set: CommitSet = match commits {
        Some(commits) => commits.into_iter().collect(),
        None => dag.query_obsolete_commits(),
    };
    // Don't use `sort_commit_set` since the set of obsolete commits may be very
    // large and we'll be throwing away most of them.
    let commits = dag.commit_set_to_vec(&commit_set)?;

    struct RebaseInfo {
        dest_oid: NonZeroOid,
        abandoned_child_oids: Vec<NonZeroOid>,
    }
    let rebases: Vec<RebaseInfo> = {
        let mut result = Vec::new();
        for original_commit_oid in commits {
            let abandoned_children =
                find_abandoned_children(dag, event_replayer, event_cursor, original_commit_oid)?;
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
        let permissions = match RebasePlanPermissions::verify_rewrite_set(
            dag,
            build_options,
            &rebases
                .iter()
                .flat_map(
                    |RebaseInfo {
                         dest_oid: _,
                         abandoned_child_oids,
                     }| abandoned_child_oids.iter().copied(),
                )
                .collect(),
        )? {
            Ok(permissions) => permissions,
            Err(err) => {
                err.describe(effects, &repo, dag)?;
                return Ok(Err(ExitCode(1)));
            }
        };
        let mut builder = RebasePlanBuilder::new(dag, permissions);
        for RebaseInfo {
            dest_oid,
            abandoned_child_oids,
        } in rebases
        {
            for child_oid in abandoned_child_oids {
                builder.move_subtree(child_oid, vec![dest_oid])?;
            }
        }
        match builder.build(effects, thread_pool, repo_pool)? {
            Ok(Some(rebase_plan)) => rebase_plan,
            Ok(None) => {
                writeln!(
                    effects.get_output_stream(),
                    "No abandoned commits to restack."
                )?;
                return Ok(Ok(()));
            }
            Err(err) => {
                err.describe(effects, &repo, dag)?;
                return Ok(Err(ExitCode(1)));
            }
        }
    };

    let execute_rebase_plan_result = execute_rebase_plan(
        effects,
        git_run_info,
        &repo,
        event_log_db,
        &rebase_plan,
        execute_options,
    )?;
    match execute_rebase_plan_result {
        ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ } => {
            writeln!(effects.get_output_stream(), "Finished restacking commits.")?;
            Ok(Ok(()))
        }

        ExecuteRebasePlanResult::DeclinedToMerge { failed_merge_info } => {
            failed_merge_info.describe(effects, &repo, merge_conflict_remediation)?;
            Ok(Err(ExitCode(1)))
        }

        ExecuteRebasePlanResult::Failed { exit_code } => {
            writeln!(
                effects.get_output_stream(),
                "Error: Could not restack commits (exit code {}).",
                {
                    let ExitCode(exit_code) = exit_code;
                    exit_code
                }
            )?;
            writeln!(
                effects.get_output_stream(),
                "You can resolve the error and try running `git restack` again."
            )?;
            Ok(Err(exit_code))
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
) -> EyreExitOr<()> {
    let event_replayer = EventReplayer::from_event_log_db(effects, repo, event_log_db)?;

    let mut rewritten_oids = HashMap::new();
    for branch in repo.get_all_local_branches()? {
        let branch_target = match branch.get_oid()? {
            Some(branch_target) => branch_target,
            None => {
                warn!(
                    branch_name = ?branch.get_reference_name()?,
                    "Branch was not a direct reference, could not resolve target"
                );
                continue;
            }
        };

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
    Ok(Ok(()))
}

/// Restack all abandoned commits.
///
/// Returns an exit code (0 denotes successful exit).
#[instrument]
pub fn restack(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    revsets: Vec<Revset>,
    resolve_revset_options: &ResolveRevsetOptions,
    move_options: &MoveOptions,
    merge_conflict_remediation: MergeConflictRemediation,
) -> EyreExitOr<()> {
    let now = SystemTime::now();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "restack")?;

    let references_snapshot = repo.get_references_snapshot()?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let commit_sets =
        match resolve_commits(effects, &repo, &mut dag, &revsets, resolve_revset_options) {
            Ok(commit_sets) => commit_sets,
            Err(err) => {
                err.describe(effects)?;
                return Ok(Err(ExitCode(1)));
            }
        };
    let commits: Option<HashSet<NonZeroOid>> = if commit_sets.is_empty() {
        None
    } else {
        Some(
            dag.commit_set_to_vec(&union_all(&commit_sets))?
                .into_iter()
                .collect(),
        )
    };

    let MoveOptions {
        force_rewrite_public_commits,
        force_in_memory,
        force_on_disk,
        detect_duplicate_commits_via_patch_id,
        resolve_merge_conflicts,
        dump_rebase_constraints,
        dump_rebase_plan,
        ref sign_options,
    } = *move_options;
    let build_options = BuildRebasePlanOptions {
        force_rewrite_public_commits,
        dump_rebase_constraints,
        dump_rebase_plan,
        detect_duplicate_commits_via_patch_id,
    };
    let execute_options = ExecuteRebasePlanOptions {
        now,
        event_tx_id,
        preserve_timestamps: get_restack_preserve_timestamps(&repo)?,
        force_in_memory,
        force_on_disk,
        resolve_merge_conflicts,
        check_out_commit_options: CheckOutCommitOptions {
            additional_args: Default::default(),
            reset: false,
            render_smartlog: false,
        },
        sign_option: sign_options.to_owned().into(),
    };
    let pool = ThreadPoolBuilder::new().build()?;
    let repo_pool = RepoResource::new_pool(&repo)?;

    try_exit_code!(restack_commits(
        effects,
        &pool,
        &repo_pool,
        &dag,
        &event_replayer,
        &event_log_db,
        event_cursor,
        git_run_info,
        commits,
        build_options,
        &execute_options,
        merge_conflict_remediation,
    )?);

    try_exit_code!(restack_branches(
        effects,
        &repo,
        &conn,
        git_run_info,
        &event_log_db,
        &execute_options,
    )?);

    smartlog(effects, git_run_info, Default::default())
}
