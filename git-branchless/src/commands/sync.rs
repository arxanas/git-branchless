//! Implements the `git sync` command.

use cursive_core::theme::BaseColor;
use lib::try_exit_code;
use std::fmt::Write;
use std::time::SystemTime;

use itertools::Itertools;
use lib::core::check_out::CheckOutCommitOptions;
use lib::core::repo_ext::RepoExt;
use lib::util::{ExitCode, EyreExitOr};
use rayon::{ThreadPool, ThreadPoolBuilder};

use git_branchless_opts::{MoveOptions, ResolveRevsetOptions, Revset};
use git_branchless_revset::{check_revset_syntax, resolve_commits};
use lib::core::config::get_restack_preserve_timestamps;
use lib::core::dag::{sorted_commit_set, union_all, CommitSet, Dag};
use lib::core::effects::{Effects, OperationType, WithProgress};
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::{Pluralize, StyledStringBuilder};
use lib::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanError, BuildRebasePlanOptions, ExecuteRebasePlanOptions,
    ExecuteRebasePlanResult, FailedMergeInfo, RebasePlan, RebasePlanBuilder, RebasePlanPermissions,
    RepoPool, RepoResource,
};
use lib::core::task::ResourcePool;
use lib::git::{
    CategorizedReferenceName, Commit, GitRunInfo, NonZeroOid, Repo, ResolvedReferenceInfo,
};

fn get_stack_roots(dag: &Dag, commit_sets: Vec<CommitSet>) -> eyre::Result<CommitSet> {
    let draft_commits = dag.query_draft_commits()?;

    // FIXME: if two draft roots are ancestors of a single commit (due to a
    // merge commit), then the entire unit should be treated as one stack and
    // moved together, rather than attempting two separate rebases.
    let draft_roots = dag.query_roots(draft_commits.clone())?;

    if commit_sets.is_empty() {
        return Ok(draft_roots);
    }

    let stack_ancestors = dag.query_range(draft_roots, union_all(&commit_sets))?;
    dag.query_roots(stack_ancestors)
        .map_err(|err| eyre::eyre!("Could not query DAG for stack roots: {err}"))
}

/// Move all commit stacks on top of the main branch.
pub fn sync(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    pull: bool,
    move_options: &MoveOptions,
    revsets: Vec<Revset>,
    resolve_revset_options: &ResolveRevsetOptions,
) -> EyreExitOr<()> {
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "sync fetch")?;

    // Try to surface parse errors early, before potentially doing commit graph or network
    // side-effects.
    check_revset_syntax(&repo, &revsets)?;

    if pull {
        try_exit_code!(git_run_info.run(effects, Some(event_tx_id), &["fetch", "--all"])?);
    }

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
            reset: false,
            render_smartlog: false,
        },
        sign_option: sign_options.to_owned().into(),
    };
    let thread_pool = ThreadPoolBuilder::new().build()?;
    let repo_pool = RepoResource::new_pool(&repo)?;

    let head_info = repo.get_head_info()?;
    if pull {
        try_exit_code!(execute_main_branch_sync_plan(
            effects,
            git_run_info,
            &repo,
            &event_log_db,
            &build_options,
            &execute_options,
            &thread_pool,
            &repo_pool,
            &head_info,
        )?);
    }

    // The main branch might have changed since we synced with `master`, so read its information again.

    execute_sync_plans(
        effects,
        git_run_info,
        &repo,
        &event_log_db,
        build_options,
        &execute_options,
        &thread_pool,
        &repo_pool,
        revsets,
        resolve_revset_options,
    )
}

fn execute_main_branch_sync_plan(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_log_db: &EventLogDb,
    build_options: &BuildRebasePlanOptions,
    execute_options: &ExecuteRebasePlanOptions,
    thread_pool: &ThreadPool,
    repo_pool: &RepoPool,
    head_info: &ResolvedReferenceInfo,
) -> EyreExitOr<()> {
    let event_replayer = EventReplayer::from_event_log_db(effects, repo, event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let references_snapshot = repo.get_references_snapshot()?;
    let mut dag = Dag::open_and_sync(
        effects,
        repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let local_main_branch = repo.get_main_branch()?;
    let local_main_branch_oid = local_main_branch.get_oid()?;
    let local_main_branch_reference_name = local_main_branch.get_reference_name()?;
    let local_main_branch_description = effects.get_glyphs().render(
        StyledStringBuilder::new()
            .append_styled(
                CategorizedReferenceName::new(&local_main_branch_reference_name)
                    .friendly_describe(),
                BaseColor::Green.dark(),
            )
            .build(),
    )?;

    let upstream_main_branch = match local_main_branch.get_upstream_branch()? {
        Some(upstream_main_branch) => upstream_main_branch,
        None => {
            writeln!(
                effects.get_output_stream(),
                "{local_main_branch_description} does not track an upstream branch, so not pulling."
            )?;
            return Ok(Ok(()));
        }
    };
    let upstream_main_branch_oid = match upstream_main_branch.get_oid()? {
        Some(upstream_main_branch_oid) => upstream_main_branch_oid,
        None => return Ok(Ok(())),
    };
    dag.sync_from_oids(
        effects,
        repo,
        CommitSet::from(upstream_main_branch_oid),
        CommitSet::empty(),
    )?;
    let local_main_branch_commits = dag.query_only(
        local_main_branch_oid.into_iter().collect(),
        CommitSet::from(upstream_main_branch_oid),
    )?;
    if dag.set_is_empty(&local_main_branch_commits)? {
        if local_main_branch_oid == Some(upstream_main_branch_oid) {
            let local_main_branch_commit = repo.find_commit_or_fail(upstream_main_branch_oid)?;
            writeln!(
                effects.get_output_stream(),
                "Not updating {} at {}",
                local_main_branch_description,
                effects
                    .get_glyphs()
                    .render(local_main_branch_commit.friendly_describe(effects.get_glyphs())?)?,
            )?;
        } else {
            let remote_main_branch_commit = repo.find_commit_or_fail(upstream_main_branch_oid)?;
            writeln!(
                effects.get_output_stream(),
                "Fast-forwarding {} to {}",
                local_main_branch_description,
                effects
                    .get_glyphs()
                    .render(remote_main_branch_commit.friendly_describe(effects.get_glyphs())?)?,
            )?;
        }

        if head_info.reference_name.as_ref() == Some(&local_main_branch_reference_name) {
            // If the main branch is checked out, make sure to update the index
            // as well as the reference itself. Otherwise, staged changes will
            // appear in `git status`.
            //
            // FIXME: this is suboptimal since it relies on the working copy.
            // However, handling e.g. dirty working copy changes is error-prone
            // and I haven't implemented it for now.
            try_exit_code!(git_run_info.run(
                effects,
                Some(execute_options.event_tx_id),
                &["rebase", &upstream_main_branch_oid.to_string()],
            )?);
        } else {
            repo.create_reference(
                &local_main_branch_reference_name,
                upstream_main_branch_oid,
                true,
                "sync",
            )?;
        }

        return Ok(Ok(()));
    } else {
        writeln!(
            effects.get_output_stream(),
            "Syncing {local_main_branch_description}"
        )?;
    }

    let build_options = BuildRebasePlanOptions {
        // Since we're syncing the main branch, by definition, any commits on it would be public, so
        // we need to set this to `true` to get the rebase to succeed.
        force_rewrite_public_commits: true,
        ..build_options.clone()
    };
    let permissions = match RebasePlanPermissions::verify_rewrite_set(
        &dag,
        build_options,
        &local_main_branch_commits,
    )? {
        Ok(permissions) => permissions,
        Err(err) => {
            err.describe(effects, repo, &dag)?;
            return Ok(Err(ExitCode(1)));
        }
    };
    let mut builder = RebasePlanBuilder::new(&dag, permissions);
    let local_main_branch_roots = dag.query_roots(local_main_branch_commits)?;
    let root_commit_oid = match dag
        .commit_set_to_vec(&local_main_branch_roots)?
        .into_iter()
        .exactly_one()
    {
        Ok(root_oid) => root_oid,
        Err(_) => return Ok(Ok(())),
    };
    builder.move_subtree(root_commit_oid, vec![upstream_main_branch_oid])?;
    let rebase_plan = match builder.build(effects, thread_pool, repo_pool)? {
        Ok(rebase_plan) => rebase_plan,
        Err(err) => {
            err.describe(effects, repo, &dag)?;
            return Ok(Err(ExitCode(1)));
        }
    };
    let rebase_plan = match rebase_plan {
        Some(rebase_plan) => rebase_plan,
        None => return Ok(Ok(())),
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
    build_options: BuildRebasePlanOptions,
    execute_options: &ExecuteRebasePlanOptions,
    thread_pool: &ThreadPool,
    repo_pool: &ResourcePool<RepoResource>,
    revsets: Vec<Revset>,
    resolve_revset_options: &ResolveRevsetOptions,
) -> EyreExitOr<()> {
    let event_replayer = EventReplayer::from_event_log_db(effects, repo, event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let references_snapshot = repo.get_references_snapshot()?;
    let mut dag = Dag::open_and_sync(
        effects,
        repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;
    let commit_sets =
        match resolve_commits(effects, repo, &mut dag, &revsets, resolve_revset_options) {
            Ok(commit_sets) => commit_sets,
            Err(err) => {
                err.describe(effects)?;
                return Ok(Err(ExitCode(1)));
            }
        };
    let main_branch_oid = repo.get_main_branch_oid()?;
    let root_commit_oids = get_stack_roots(&dag, commit_sets)?;
    let root_commits = sorted_commit_set(repo, &dag, &root_commit_oids)?;
    let permissions =
        match RebasePlanPermissions::verify_rewrite_set(&dag, build_options, &root_commit_oids)? {
            Ok(permissions) => permissions,
            Err(err) => {
                err.describe(effects, repo, &dag)?;
                return Ok(Err(ExitCode(1)));
            }
        };
    let builder = RebasePlanBuilder::new(&dag, permissions);

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
            err.describe(effects, repo, &dag)?;
            return Ok(Err(ExitCode(1)));
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
) -> EyreExitOr<()> {
    let (success_commits, failed_merge_commits, skipped_commits) = {
        let mut success_commits: Vec<Commit> = Vec::new();
        let mut failed_merge_commits: Vec<(Commit, FailedMergeInfo)> = Vec::new();
        let mut skipped_commits: Vec<Commit> = Vec::new();

        let (effects, progress) = effects.start_operation(OperationType::SyncCommits);
        for (root_commit_oid, rebase_plan) in
            root_commit_and_plans.into_iter().with_progress(progress)
        {
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
            match result {
                ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ } => {
                    success_commits.push(root_commit);
                }
                ExecuteRebasePlanResult::DeclinedToMerge { failed_merge_info } => {
                    failed_merge_commits.push((root_commit, failed_merge_info));
                }
                ExecuteRebasePlanResult::Failed { exit_code } => {
                    return Ok(Err(exit_code));
                }
            }
        }

        (success_commits, failed_merge_commits, skipped_commits)
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

    for (failed_merge_commit, failed_merge_info) in failed_merge_commits {
        match failed_merge_info {
            FailedMergeInfo::Conflict {
                commit_oid: _,
                conflicting_paths,
            } => {
                writeln!(
                    effects.get_output_stream(),
                    "Merge conflict ({}) for {}",
                    Pluralize {
                        determiner: None,
                        amount: conflicting_paths.len(),
                        unit: ("file", "files")
                    },
                    effects.get_glyphs().render(
                        StyledStringBuilder::new()
                            .append(failed_merge_commit.friendly_describe(effects.get_glyphs())?)
                            .build()
                    )?
                )?;
            }
            FailedMergeInfo::CannotRebaseMergeInMemory { commit_oid: _ } => {
                writeln!(
                    effects.get_output_stream(),
                    "Can't rebase merge commit in-memory: {}",
                    effects.get_glyphs().render(
                        StyledStringBuilder::new()
                            .append(failed_merge_commit.friendly_describe(effects.get_glyphs())?)
                            .build()
                    )?
                )?;
            }
        }
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

    Ok(Ok(()))
}
