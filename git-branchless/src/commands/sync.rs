//! Implements the `git sync` command.

use cursive::theme::BaseColor;
use std::fmt::Write;
use std::time::SystemTime;

use eden_dag::DagAlgorithm;
use eyre::Report;
use itertools::Itertools;
use lib::core::check_out::CheckOutCommitOptions;
use lib::core::repo_ext::RepoExt;
use lib::util::ExitCode;
use rayon::{ThreadPool, ThreadPoolBuilder};

use crate::opts::{MoveOptions, Revset};
use crate::revset::resolve_commits;
use lib::core::config::get_restack_preserve_timestamps;
use lib::core::dag::{commit_set_to_vec, sorted_commit_set, union_all, CommitSet, Dag};
use lib::core::effects::{Effects, OperationType};
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::StyledStringBuilder;
use lib::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanError, BuildRebasePlanOptions, ExecuteRebasePlanOptions,
    ExecuteRebasePlanResult, RebasePlan, RebasePlanBuilder, RebasePlanPermissions, RepoPool,
    RepoResource,
};
use lib::core::task::ResourcePool;
use lib::git::{CategorizedReferenceName, Commit, GitRunInfo, NonZeroOid, Repo};

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
    pull: bool,
    move_options: &MoveOptions,
    revsets: Vec<Revset>,
) -> eyre::Result<ExitCode> {
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "sync fetch")?;

    if pull {
        let exit_code = git_run_info.run(effects, Some(event_tx_id), &["fetch", "--all"])?;
        if !exit_code.is_success() {
            return Ok(exit_code);
        }
    }

    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let references_snapshot = repo.get_references_snapshot()?;
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let commit_sets = match resolve_commits(effects, &repo, &mut dag, &revsets) {
        Ok(commit_sets) => commit_sets,
        Err(err) => {
            err.describe(effects)?;
            return Ok(ExitCode(1));
        }
    };
    let root_commit_oids = if commit_sets.is_empty() {
        get_stack_roots(&dag)?
    } else {
        dag.query().roots(union_all(&commit_sets))?
    };
    let root_commits = sorted_commit_set(&repo, &dag, &root_commit_oids)?;

    let MoveOptions {
        force_rewrite_public_commits,
        force_in_memory,
        force_on_disk,
        detect_duplicate_commits_via_patch_id,
        resolve_merge_conflicts,
        dump_rebase_constraints,
        dump_rebase_plan,
    } = *move_options;
    let build_options = BuildRebasePlanOptions {
        force_rewrite_public_commits,
        detect_duplicate_commits_via_patch_id,
        dump_rebase_constraints,
        dump_rebase_plan,
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
        check_out_commit_options: CheckOutCommitOptions {
            additional_args: Default::default(),
            render_smartlog: false,
        },
    };
    let thread_pool = ThreadPoolBuilder::new().build()?;
    let repo_pool = RepoResource::new_pool(&repo)?;

    if pull {
        let exit_code = execute_main_branch_sync_plan(
            effects,
            git_run_info,
            &repo,
            &mut dag,
            &event_log_db,
            &build_options,
            &execute_options,
            &thread_pool,
            &repo_pool,
        )?;
        if !exit_code.is_success() {
            return Ok(exit_code);
        }
    }

    // The main branch OID might have changed since we synced with `master`, so read it again.
    let main_branch_oid = repo.get_main_branch_oid()?;
    execute_sync_plans(
        effects,
        git_run_info,
        &repo,
        &event_log_db,
        &mut dag,
        &root_commit_oids,
        root_commits,
        main_branch_oid,
        &build_options,
        &execute_options,
        &thread_pool,
        &repo_pool,
    )
}

fn execute_main_branch_sync_plan(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    dag: &mut Dag,
    event_log_db: &EventLogDb,
    build_options: &BuildRebasePlanOptions,
    execute_options: &ExecuteRebasePlanOptions,
    thread_pool: &ThreadPool,
    repo_pool: &RepoPool,
) -> eyre::Result<ExitCode> {
    let main_branch = repo.get_main_branch()?;
    let upstream_main_branch = match main_branch.get_upstream_branch()? {
        Some(upstream_main_branch) => upstream_main_branch,
        None => return Ok(ExitCode(0)),
    };
    let upstream_main_branch_oid = match upstream_main_branch.get_oid()? {
        Some(upstream_main_branch_oid) => upstream_main_branch_oid,
        None => return Ok(ExitCode(0)),
    };
    dag.sync_from_oids(
        effects,
        repo,
        CommitSet::from(upstream_main_branch_oid),
        CommitSet::empty(),
    )?;
    let local_main_branch_commits = dag.query().only(
        main_branch.get_oid()?.into_iter().collect(),
        CommitSet::from(upstream_main_branch_oid),
    )?;

    let main_branch_reference_name = main_branch.get_reference_name()?;
    let branch_description = effects.get_glyphs().render(
        StyledStringBuilder::new()
            .append_styled(
                CategorizedReferenceName::new(&main_branch_reference_name).friendly_describe(),
                BaseColor::Green.dark(),
            )
            .build(),
    )?;
    if local_main_branch_commits.is_empty()? {
        writeln!(
            effects.get_output_stream(),
            "Fast-forwarding {}",
            branch_description
        )?;
        repo.create_reference(
            &main_branch_reference_name,
            upstream_main_branch_oid,
            true,
            "sync",
        )?;
        return Ok(ExitCode(0));
    } else {
        writeln!(
            effects.get_output_stream(),
            "Syncing {}",
            branch_description
        )?;
    }

    let build_options = BuildRebasePlanOptions {
        // Since we're syncing the main branch, by definition, any commits on it would be public, so
        // we need to set this to `true` to get the rebase to succeed.
        force_rewrite_public_commits: true,
        ..build_options.clone()
    };
    let permissions = match RebasePlanPermissions::verify_rewrite_set(
        dag,
        &build_options,
        &local_main_branch_commits,
    )? {
        Ok(permissions) => permissions,
        Err(err) => {
            err.describe(effects, repo)?;
            return Ok(ExitCode(1));
        }
    };
    let mut builder = RebasePlanBuilder::new(dag, permissions);
    let local_main_branch_roots = dag.query().roots(local_main_branch_commits)?;
    let root_commit_oid = match commit_set_to_vec(&local_main_branch_roots)?
        .into_iter()
        .exactly_one()
    {
        Ok(root_oid) => root_oid,
        Err(_) => return Ok(ExitCode(0)),
    };
    builder.move_subtree(root_commit_oid, vec![upstream_main_branch_oid])?;
    let rebase_plan = match builder.build(effects, thread_pool, repo_pool)? {
        Ok(rebase_plan) => rebase_plan,
        Err(err) => {
            err.describe(effects, repo)?;
            return Ok(ExitCode(1));
        }
    };
    let rebase_plan = match rebase_plan {
        Some(rebase_plan) => rebase_plan,
        None => return Ok(ExitCode(0)),
    };

    execute_plans(
        effects,
        git_run_info,
        repo,
        event_log_db,
        execute_options,
        vec![(root_commit_oid, Some(rebase_plan))],
    )
}

fn execute_sync_plans(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_log_db: &EventLogDb,
    dag: &mut Dag,
    root_commit_oids: &CommitSet,
    root_commits: Vec<Commit>,
    main_branch_oid: NonZeroOid,
    build_options: &BuildRebasePlanOptions,
    execute_options: &ExecuteRebasePlanOptions,
    thread_pool: &ThreadPool,
    repo_pool: &ResourcePool<RepoResource>,
) -> eyre::Result<ExitCode> {
    let permissions =
        match RebasePlanPermissions::verify_rewrite_set(dag, build_options, root_commit_oids)? {
            Ok(permissions) => permissions,
            Err(err) => {
                err.describe(effects, repo)?;
                return Ok(ExitCode(1));
            }
        };
    let builder = RebasePlanBuilder::new(dag, permissions);

    let root_commit_oids = root_commits
        .into_iter()
        .map(|commit| commit.get_oid())
        .collect_vec();
    let root_commit_and_plans = thread_pool.install(|| -> eyre::Result<_> {
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

                    let only_parent_id =
                        root_commit.get_only_parent().map(|parent| parent.get_oid());
                    if only_parent_id == Some(main_branch_oid) {
                        return Ok(Ok((root_commit_oid, None)));
                    }

                    builder.move_subtree(root_commit.get_oid(), vec![main_branch_oid])?;
                    let rebase_plan = builder.build(effects, thread_pool, repo_pool)?;
                    Ok(rebase_plan.map(|rebase_plan| (root_commit_oid, rebase_plan)))
                },
            )
            .collect::<eyre::Result<Vec<_>>>()?
            .into_iter()
            .collect::<Result<Vec<_>, BuildRebasePlanError>>();
        Ok(result)
    })?;

    let root_commit_and_plans = match root_commit_and_plans {
        Ok(root_commit_and_plans) => root_commit_and_plans,
        Err(err) => {
            err.describe(effects, repo)?;
            return Ok(ExitCode(1));
        }
    };
    execute_plans(
        effects,
        git_run_info,
        repo,
        event_log_db,
        execute_options,
        root_commit_and_plans,
    )
}

fn execute_plans(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_log_db: &EventLogDb,
    execute_options: &ExecuteRebasePlanOptions,
    root_commit_and_plans: Vec<(NonZeroOid, Option<RebasePlan>)>,
) -> Result<ExitCode, Report> {
    let (success_commits, merge_conflict_commits, skipped_commits) = {
        let mut success_commits: Vec<Commit> = Vec::new();
        let mut merge_conflict_commits: Vec<Commit> = Vec::new();
        let mut skipped_commits: Vec<Commit> = Vec::new();

        let (effects, progress) = effects.start_operation(OperationType::SyncCommits);
        progress.notify_progress(0, root_commit_and_plans.len());

        for (root_commit_oid, rebase_plan) in root_commit_and_plans {
            let root_commit = repo.find_commit_or_fail(root_commit_oid)?;
            let rebase_plan = match rebase_plan {
                Some(rebase_plan) => rebase_plan,
                None => {
                    skipped_commits.push(root_commit);
                    continue;
                }
            };

            let result = execute_rebase_plan(
                &effects,
                git_run_info,
                repo,
                event_log_db,
                &rebase_plan,
                execute_options,
            )?;
            progress.notify_progress_inc(1);
            match result {
                ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ } => {
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

        (success_commits, merge_conflict_commits, skipped_commits)
    };

    for success_commit in success_commits {
        writeln!(
            effects.get_output_stream(),
            "{}",
            effects.get_glyphs().render(
                StyledStringBuilder::new()
                    .append_plain("Synced ")
                    .append(success_commit.friendly_describe(effects.get_glyphs())?)
                    .build()
            )?
        )?;
    }

    for merge_conflict_commit in merge_conflict_commits {
        writeln!(
            effects.get_output_stream(),
            "Merge conflict for {}",
            effects.get_glyphs().render(
                StyledStringBuilder::new()
                    .append(merge_conflict_commit.friendly_describe(effects.get_glyphs())?)
                    .build()
            )?
        )?;
    }

    for skipped_commit in skipped_commits {
        writeln!(
            effects.get_output_stream(),
            "Not moving up-to-date stack at {}",
            effects
                .get_glyphs()
                .render(skipped_commit.friendly_describe(effects.get_glyphs())?)?
        )?;
    }

    Ok(ExitCode(0))
}
