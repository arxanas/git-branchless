//! Implements the `git sync` command.

use std::fmt::Write;
use std::time::SystemTime;

use eden_dag::DagAlgorithm;
use itertools::Itertools;
use rayon::ThreadPoolBuilder;

use crate::core::config::get_restack_preserve_timestamps;
use crate::core::dag::{sort_commit_set, CommitSet, Dag};
use crate::core::effects::{Effects, OperationType};
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::formatting::{printable_styled_string, Glyphs, StyledStringBuilder};
use crate::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanError, BuildRebasePlanOptions, ExecuteRebasePlanOptions,
    ExecuteRebasePlanResult, RebasePlan, RebasePlanBuilder, RepoResource,
};
use crate::git::{Commit, GitRunInfo, NonZeroOid, Repo};
use crate::opts::MoveOptions;

fn get_stack_roots(dag: &Dag) -> eyre::Result<CommitSet> {
    let public_commits = dag.query_public_commits()?;
    let active_heads = dag.query_active_heads(
        &public_commits,
        &dag.observed_commits.difference(&dag.obsolete_commits),
    )?;
    let draft_commits = dag
        .query()
        .range(public_commits.clone(), active_heads)?
        .difference(&public_commits);

    // FIXME: if two draft roots are ancestors of a single commit (due to a
    // merge commit), then the entire unit should be treated as one stack and
    // moved together, rather than attempting two separate rebases.
    let draft_roots = dag.query().roots(draft_commits)?;
    Ok(draft_roots)
}

/// Move all commit stacks on top of the main branch.
pub fn sync(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    update_refs: bool,
    move_options: &MoveOptions,
) -> eyre::Result<isize> {
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "sync fetch")?;

    if update_refs {
        let exit_code = git_run_info.run(effects, Some(event_tx_id), &["fetch"])?;
        if exit_code != 0 {
            return Ok(exit_code);
        }
    }

    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let references_snapshot = repo.get_references_snapshot()?;
    let dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let root_commits = get_stack_roots(&dag)?;
    let root_commits = sort_commit_set(&repo, &dag, &root_commits)?;

    let MoveOptions {
        force_in_memory,
        force_on_disk,
        resolve_merge_conflicts,
        dump_rebase_constraints,
        dump_rebase_plan,
    } = *move_options;
    let pool = ThreadPoolBuilder::new().build()?;
    let repo_pool = RepoResource::new_pool(&repo)?;
    let root_commit_and_plans: Vec<(NonZeroOid, Option<RebasePlan>)> = {
        let builder = RebasePlanBuilder::new(&dag);
        let root_commit_oids = root_commits
            .into_iter()
            .map(|commit| commit.get_oid())
            .collect_vec();
        let root_commit_and_plans = pool.install(|| -> eyre::Result<_> {
            let result = root_commit_oids
                // Don't parallelize for now, since the status updates don't render well.
                .into_iter()
                .map(
                    |root_commit_oid| -> eyre::Result<
                        Result<(NonZeroOid, Option<RebasePlan>), BuildRebasePlanError>,
                    > {
                        // Keep access to the same underlying caches by cloning the same instance of the builder.
                        let mut builder = builder.clone();

                        let repo = repo_pool.try_create()?;
                        let root_commit = repo.find_commit_or_fail(root_commit_oid)?;

                        builder.move_subtree(
                            root_commit.get_oid(),
                            references_snapshot.main_branch_oid,
                        )?;
                        let rebase_plan = builder.build(
                            effects,
                            &pool,
                            &repo_pool,
                            &BuildRebasePlanOptions {
                                detect_duplicate_commits_via_patch_id: true,
                                dump_rebase_constraints,
                                dump_rebase_plan,
                            },
                        )?;
                        Ok(rebase_plan.map(|rebase_plan| (root_commit_oid, rebase_plan)))
                    },
                )
                .collect::<eyre::Result<Vec<_>>>()?
                .into_iter()
                .collect::<Result<Vec<_>, BuildRebasePlanError>>();
            Ok(result)
        })?;

        match root_commit_and_plans {
            Ok(root_commit_and_plans) => root_commit_and_plans,
            Err(err) => {
                err.describe(effects, &repo)?;
                return Ok(1);
            }
        }
    };

    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "sync")?;
    let execute_options = ExecuteRebasePlanOptions {
        now,
        event_tx_id,
        preserve_timestamps: get_restack_preserve_timestamps(&repo)?,
        force_in_memory,
        force_on_disk,
        resolve_merge_conflicts,
    };

    let (success_commits, merge_conflict_commits) = {
        let mut success_commits: Vec<Commit> = Vec::new();
        let mut merge_conflict_commits: Vec<Commit> = Vec::new();

        let (effects, progress) = effects.start_operation(OperationType::SyncCommits);
        progress.notify_progress(0, root_commit_and_plans.len());

        for (root_commit_oid, rebase_plan) in root_commit_and_plans {
            let root_commit = repo.find_commit_or_fail(root_commit_oid)?;
            let rebase_plan = match rebase_plan {
                Some(rebase_plan) => rebase_plan,
                None => {
                    writeln!(
                        effects.get_output_stream(),
                        "Not moving up-to-date stack at {}",
                        printable_styled_string(&glyphs, root_commit.friendly_describe()?)?
                    )?;
                    continue;
                }
            };

            let result = execute_rebase_plan(
                &effects,
                git_run_info,
                &repo,
                &rebase_plan,
                &execute_options,
            )?;
            progress.notify_progress_inc(1);
            match result {
                ExecuteRebasePlanResult::Succeeded => {
                    success_commits.push(root_commit);
                }
                ExecuteRebasePlanResult::DeclinedToMerge { merge_conflict: _ } => {
                    merge_conflict_commits.push(root_commit);
                }
                ExecuteRebasePlanResult::Failed { exit_code } => {
                    return Ok(exit_code);
                }
            }
        }

        (success_commits, merge_conflict_commits)
    };

    for success_commit in success_commits {
        writeln!(
            effects.get_output_stream(),
            "{}",
            printable_styled_string(
                &glyphs,
                StyledStringBuilder::new()
                    .append_plain("Synced ")
                    .append(success_commit.friendly_describe()?)
                    .build()
            )?
        )?;
    }

    for merge_conflict_commit in merge_conflict_commits {
        writeln!(
            effects.get_output_stream(),
            "{}",
            printable_styled_string(
                &glyphs,
                StyledStringBuilder::new()
                    .append_plain("Merge conflict for ")
                    .append(merge_conflict_commit.friendly_describe()?)
                    .build()
            )?
        )?;
    }

    Ok(0)
}
