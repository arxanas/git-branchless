//! Split commits, extracting changes from a single commit into separate commits.

use eyre::Context;
use rayon::ThreadPoolBuilder;
use std::{
    fmt::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use git_branchless_opts::{MoveOptions, ResolveRevsetOptions, Revset};
use git_branchless_revset::resolve_commits;
use lib::{
    core::{
        check_out::{check_out_commit, CheckOutCommitOptions, CheckoutTarget},
        config::get_restack_preserve_timestamps,
        dag::{CommitSet, Dag},
        effects::Effects,
        eventlog::{Event, EventLogDb, EventReplayer},
        gc::mark_commit_reachable,
        repo_ext::RepoExt,
        rewrite::{
            execute_rebase_plan, move_branches, BuildRebasePlanOptions, ExecuteRebasePlanOptions,
            ExecuteRebasePlanResult, MergeConflictRemediation, RebasePlanBuilder,
            RebasePlanPermissions, RepoResource,
        },
    },
    git::{
        make_empty_tree, summarize_diff_for_temporary_commit, CherryPickFastOptions, GitRunInfo,
        MaybeZeroOid, NonZeroOid, Repo, ResolvedReferenceInfo,
    },
    try_exit_code,
    util::{ExitCode, EyreExitOr},
};
use tracing::instrument;

/// Split a commit and restack its descendants.
#[instrument]
pub fn split(
    effects: &Effects,
    revset: Revset,
    resolve_revset_options: &ResolveRevsetOptions,
    files_to_extract: Vec<String>,
    detach: bool,
    move_options: &MoveOptions,
    git_run_info: &GitRunInfo,
) -> EyreExitOr<()> {
    let repo = Repo::from_current_dir()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;
    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "split")?;
    let pool = ThreadPoolBuilder::new().build()?;
    let repo_pool = RepoResource::new_pool(&repo)?;

    let MoveOptions {
        force_rewrite_public_commits,
        force_in_memory,
        force_on_disk,
        detect_duplicate_commits_via_patch_id,
        resolve_merge_conflicts,
        dump_rebase_constraints,
        dump_rebase_plan,
    } = *move_options;

    let commit_to_split_oid: NonZeroOid = match resolve_commits(
        effects,
        &repo,
        &mut dag,
        &[revset.clone()],
        resolve_revset_options,
    ) {
        Ok(commit_sets) => match dag.commit_set_to_vec(&commit_sets[0])?.as_slice() {
            [only_commit_oid] => *only_commit_oid,
            other => {
                let Revset(expr) = revset;
                writeln!(
                    effects.get_error_stream(),
                    "Expected revset to expand to exactly 1 commit (got {count}): {expr}",
                    count = other.len(),
                )?;
                return Ok(Err(ExitCode(1)));
            }
        },
        Err(err) => {
            err.describe(effects)?;
            return Ok(Err(ExitCode(1)));
        }
    };

    let permissions = match RebasePlanPermissions::verify_rewrite_set(
        &dag,
        BuildRebasePlanOptions {
            force_rewrite_public_commits,
            dump_rebase_constraints,
            dump_rebase_plan,
            detect_duplicate_commits_via_patch_id,
        },
        &vec![commit_to_split_oid].into_iter().collect(),
    )? {
        Ok(permissions) => permissions,
        Err(err) => {
            err.describe(effects, &repo, &dag)?;
            return Ok(Err(ExitCode(1)));
        }
    };

    let commit_to_split = repo.find_commit_or_fail(commit_to_split_oid)?;
    let parent_commits = commit_to_split.get_parents();
    let parent_tree = match parent_commits.as_slice() {
        [only_parent] => only_parent.get_tree()?,
        [] => make_empty_tree(&repo)?,
        [..] => {
            writeln!(
                effects.get_error_stream(),
                "Cannot split merge commit {}.",
                commit_to_split_oid
            )?;
            return Ok(Err(ExitCode(1)));
        }
    };

    let mut split_tree = commit_to_split.get_tree()?;
    for file in files_to_extract.iter() {
        let path = Path::new(&file).to_path_buf();
        let cwd = std::env::current_dir()?;
        let working_copy_path = match repo.get_working_copy_path() {
            Some(working_copy_path) => working_copy_path,
            None => {
                writeln!(
                    effects.get_error_stream(),
                    "Aborting. Split is not supported in bare root repositories.",
                )?;
                return Ok(Err(ExitCode(1)));
            }
        };

        let path = if cwd != working_copy_path && path.exists() {
            let mut repo_relative_path = match cwd.strip_prefix(working_copy_path) {
                Ok(working_copy_path) => working_copy_path.to_path_buf(),
                Err(_) => {
                    writeln!(
                        effects.get_error_stream(),
                        "Error: current working directory is not in the working copy.\n\
                        This may be a bug, please report it.",
                    )?;
                    return Ok(Err(ExitCode(1)));
                }
            };
            repo_relative_path.push(path);
            repo_relative_path
        } else if let Some(stripped_filename) = file.strip_prefix(":/") {
            // https://git-scm.com/docs/gitrevisions#Documentation/gitrevisions.txt-emltngtltpathgtemegem0READMEememREADMEem
            Path::new(stripped_filename).to_path_buf()
        } else {
            path
        };
        let path = path.as_path();

        if let Ok(Some(false)) = commit_to_split.contains_touched_path(path) {
            writeln!(
                effects.get_error_stream(),
                "Aborting: file '{filename}' was not changed in commit {oid}.",
                filename = path.to_string_lossy(),
                oid = commit_to_split.get_short_oid()?
            )?;
            return Ok(Err(ExitCode(1)));
        }

        let parent_entry = match parent_tree.get_path(path) {
            Ok(entry) => entry,
            Err(err) => {
                writeln!(
                    effects.get_error_stream(),
                    "uh oh error reading tree entry: {err}.",
                )?;
                return Ok(Err(ExitCode(1)));
            }
        };

        let commit_has_entry = split_tree.get_path(path)?.is_some();
        match parent_entry {
            // added => remove from commit
            None if commit_has_entry => {
                let new_split_tree_oid = split_tree.remove(&repo, path)?;
                split_tree = repo
                    .find_tree(new_split_tree_oid)?
                    .expect("should have been found");
            }

            // deleted or modified => replace w/ parent content
            Some(parent_entry) => {
                let new_split_tree_oid = split_tree.add_or_replace(&repo, path, &parent_entry)?;
                split_tree = repo
                    .find_tree(new_split_tree_oid)?
                    .expect("should have been found");
            }

            None => {
                if path.exists() {
                    writeln!(
                        effects.get_error_stream(),
                        "Aborting: the file '{file}' could not be found in this repo.\nPerhaps it's not under version control?",
                    )?;
                } else {
                    writeln!(
                        effects.get_error_stream(),
                        "Aborting: the file '{file}' does not exist.",
                    )?;
                }
                return Ok(Err(ExitCode(1)));
            }
        }
    }
    let message = {
        let (effects, _progress) =
            effects.start_operation(lib::core::effects::OperationType::CalculateDiff);
        let diff = repo.get_diff_between_trees(
            &effects,
            Some(&split_tree),
            &commit_to_split.get_tree()?,
            0, // we don't care about the context here
        )?;

        summarize_diff_for_temporary_commit(&diff)?
    };

    let split_commit_oid =
        commit_to_split.amend_commit(None, None, None, None, Some(&split_tree))?;
    let split_commit = repo.find_commit_or_fail(split_commit_oid)?;

    if split_commit.is_empty() {
        writeln!(
            effects.get_error_stream(),
            "Aborting: refusing to split all changes out of commit {oid}.",
            oid = commit_to_split.get_short_oid()?,
        )?;
        return Ok(Err(ExitCode(1)));
    };

    let extracted_tree = repo.cherry_pick_fast(
        &commit_to_split,
        &split_commit,
        &CherryPickFastOptions {
            reuse_parent_tree_if_possible: true,
        },
    )?;
    let extracted_commit_oid = repo.create_commit(
        None,
        &commit_to_split.get_author(),
        &commit_to_split.get_committer(),
        format!("temp(split): {message}").as_str(),
        &extracted_tree,
        vec![&split_commit],
    )?;

    // see git-branchless/src/commands/amend.rs:172
    // TODO maybe this should happen after we've confirmed the rebase has succeeded
    mark_commit_reachable(&repo, extracted_commit_oid)
        .wrap_err("Marking commit as reachable for GC purposes.")?;
    event_log_db.add_events(vec![Event::RewriteEvent {
        timestamp: now.duration_since(UNIX_EPOCH)?.as_secs_f64(),
        event_tx_id,
        old_commit_oid: MaybeZeroOid::NonZero(commit_to_split_oid),
        new_commit_oid: MaybeZeroOid::NonZero(split_commit_oid),
    }])?;
    event_log_db.add_events(vec![Event::CommitEvent {
        timestamp: now.duration_since(UNIX_EPOCH)?.as_secs_f64(),
        event_tx_id,
        commit_oid: extracted_commit_oid,
    }])?;

    // push the new commits into the dag for the rebase planner
    dag.sync_from_oids(
        effects,
        &repo,
        CommitSet::empty(),
        vec![split_commit_oid, extracted_commit_oid]
            .into_iter()
            .collect(),
    )?;

    let head_info = repo.get_head_info()?;
    let (checkout_target, rewritten_oids) = match head_info {
        // branch @ split commit checked out: extend branch to include extracted
        // commit; branch will stay checked out w/o any explicit checkout
        ResolvedReferenceInfo {
            oid: Some(oid),
            reference_name: Some(_),
        } if oid == commit_to_split_oid && !detach => (
            None,
            vec![(
                commit_to_split_oid,
                MaybeZeroOid::NonZero(extracted_commit_oid),
            )],
        ),

        // commit to split checked out as detached HEAD, don't extend any
        // branches, but explicitly check out the newly split commit
        ResolvedReferenceInfo {
            oid: Some(oid),
            reference_name: None,
        } if oid == commit_to_split_oid => (
            Some(CheckoutTarget::Oid(split_commit_oid)),
            vec![(commit_to_split_oid, MaybeZeroOid::NonZero(split_commit_oid))],
        ),

        // some other commit or branch was checked out, default behavior is fine
        ResolvedReferenceInfo {
            oid: _,
            reference_name: _,
        } => (
            None,
            vec![(commit_to_split_oid, MaybeZeroOid::NonZero(split_commit_oid))],
        ),
    };

    move_branches(
        effects,
        git_run_info,
        &repo,
        event_tx_id,
        &(rewritten_oids.into_iter().collect()),
    )?;

    if checkout_target.is_some() {
        try_exit_code!(check_out_commit(
            effects,
            git_run_info,
            &repo,
            &event_log_db,
            event_tx_id,
            checkout_target,
            &CheckOutCommitOptions {
                additional_args: Default::default(),
                force_detach: true,
                reset: false,
                render_smartlog: false,
            },
        )?);
    }

    let mut builder = RebasePlanBuilder::new(&dag, permissions);
    let children = dag.query_children(CommitSet::from(commit_to_split_oid))?;
    for child in dag.commit_set_to_vec(&children)? {
        if detach {
            builder.move_subtree(child, vec![split_commit_oid])?;
        } else {
            builder.move_subtree(child, vec![extracted_commit_oid])?;
        }
    }
    let rebase_plan = builder.build(effects, &pool, &repo_pool)?;

    let result = match rebase_plan {
        Ok(None) => {
            writeln!(effects.get_output_stream(), "Nothing to restack.")?;
            None
        }
        Ok(Some(rebase_plan)) => {
            let options = ExecuteRebasePlanOptions {
                now,
                event_tx_id,
                preserve_timestamps: get_restack_preserve_timestamps(&repo)?,
                force_in_memory,
                force_on_disk,
                resolve_merge_conflicts,
                check_out_commit_options: CheckOutCommitOptions {
                    additional_args: Default::default(),
                    force_detach: false,
                    reset: false,
                    render_smartlog: false,
                },
            };
            Some(execute_rebase_plan(
                effects,
                git_run_info,
                &repo,
                &event_log_db,
                &rebase_plan,
                &options,
            )?)
        }
        Err(err) => {
            err.describe(effects, &repo, &dag)?;
            return Ok(Err(ExitCode(1)));
        }
    };

    match result {
        None | Some(ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ }) => {
            try_exit_code!(git_run_info
                .run_direct_no_wrapping(Some(event_tx_id), &["branchless", "smartlog"])?);
            Ok(Ok(()))
        }

        Some(ExecuteRebasePlanResult::DeclinedToMerge { failed_merge_info }) => {
            failed_merge_info.describe(effects, &repo, MergeConflictRemediation::Retry)?;
            Ok(Err(ExitCode(1)))
        }

        Some(ExecuteRebasePlanResult::Failed { exit_code }) => Ok(Err(exit_code)),
    }
}
