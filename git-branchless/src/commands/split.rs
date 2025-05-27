//! Split commits, extracting changes from a single commit into separate commits.

use eyre::Context;
use rayon::ThreadPoolBuilder;
use std::{
    fmt::Write,
    path::{Path, PathBuf},
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

    let target_oid: NonZeroOid = match resolve_commits(
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
        &vec![target_oid].into_iter().collect(),
    )? {
        Ok(permissions) => permissions,
        Err(err) => {
            err.describe(effects, &repo, &dag)?;
            return Ok(Err(ExitCode(1)));
        }
    };

    let target_commit = repo.find_commit_or_fail(target_oid)?;
    let target_tree = target_commit.get_tree()?;
    let parent_commits = target_commit.get_parents();
    let (parent_tree, mut remainder_tree) = match parent_commits.as_slice() {
        // split the commit by removing the changes from the target, and then
        // cherry picking the orignal target as the "extracted" commit
        [only_parent] => (only_parent.get_tree()?, target_commit.get_tree()?),

        // no parent: use an empty tree for comparison
        [] => (make_empty_tree(&repo)?, target_commit.get_tree()?),

        [..] => {
            writeln!(
                effects.get_error_stream(),
                "Cannot split merge commit {}.",
                target_oid
            )?;
            return Ok(Err(ExitCode(1)));
        }
    };

    let cwd = std::env::current_dir()?;
    // tuple: (input_file, resolved_path)
    let resolved_paths_to_extract: eyre::Result<Vec<(String, PathBuf)>> = files_to_extract
        .into_iter()
        .map(|file| {
            let path = Path::new(&file).to_path_buf();
            let working_copy_path = match repo.get_working_copy_path() {
                Some(working_copy_path) => working_copy_path,
                None => {
                    eyre::bail!("Aborting. Split is not supported in bare root repositories.",)
                }
            };

            let path = if cwd != working_copy_path && path.exists() {
                let mut repo_relative_path = match cwd.strip_prefix(working_copy_path) {
                    Ok(working_copy_path) => working_copy_path.to_path_buf(),
                    Err(_) => {
                        eyre::bail!(
                            "Error: current working directory is not in the working copy.\n\
                                            This may be a bug, please report it.",
                        );
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

            Ok((file, path))
        })
        .collect();

    let resolved_paths_to_extract = match resolved_paths_to_extract {
        Ok(resolved_paths_to_extract) => resolved_paths_to_extract,
        Err(err) => {
            writeln!(effects.get_error_stream(), "{err}")?;
            return Ok(Err(ExitCode(1)));
        }
    };

    for (file, path) in resolved_paths_to_extract.iter() {
        let path = path.as_path();

        if let Ok(Some(false)) = target_commit.contains_touched_path(path) {
            writeln!(
                effects.get_error_stream(),
                "Aborting: file '{filename}' was not changed in commit {oid}.",
                filename = path.to_string_lossy(),
                oid = target_commit.get_short_oid()?
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

        let target_entry = target_tree.get_path(path)?;
        let temp_tree_oid = match (parent_entry, target_entry) {
            // added => remove from remainder commit
            (None, Some(_)) => remainder_tree.remove(&repo, path)?,

            // deleted or modified => replace w/ parent content in split commit
            (Some(parent_entry), _) => remainder_tree.add_or_replace(&repo, path, &parent_entry)?,

            (None, _) => {
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
        };

        remainder_tree = repo
            .find_tree(temp_tree_oid)?
            .expect("should have been found");
    }
    let message = {
        let (old_tree, new_tree) = (&remainder_tree, &target_tree);
        let diff = repo.get_diff_between_trees(
            effects,
            Some(old_tree),
            new_tree,
            0, // we don't care about the context here
        )?;

        summarize_diff_for_temporary_commit(&diff)?
    };

    let remainder_commit_oid =
        target_commit.amend_commit(None, None, None, None, Some(&remainder_tree))?;
    let remainder_commit = repo.find_commit_or_fail(remainder_commit_oid)?;

    if remainder_commit.is_empty() {
        writeln!(
            effects.get_error_stream(),
            "Aborting: refusing to split all changes out of commit {oid}.",
            oid = target_commit.get_short_oid()?,
        )?;
        return Ok(Err(ExitCode(1)));
    };

    event_log_db.add_events(vec![Event::RewriteEvent {
        timestamp: now.duration_since(UNIX_EPOCH)?.as_secs_f64(),
        event_tx_id,
        old_commit_oid: MaybeZeroOid::NonZero(target_oid),
        new_commit_oid: MaybeZeroOid::NonZero(remainder_commit_oid),
    }])?;

    let extracted_commit_oid = {
        let extracted_tree = repo.cherry_pick_fast(
            &target_commit,
            &remainder_commit,
            &CherryPickFastOptions {
                reuse_parent_tree_if_possible: true,
            },
        )?;
        let extracted_commit_oid = repo.create_commit(
            None,
            &target_commit.get_author(),
            &target_commit.get_committer(),
            format!("temp(split): {message}").as_str(),
            &extracted_tree,
            vec![&remainder_commit],
        )?;

        // see git-branchless/src/commands/amend.rs:172
        // TODO maybe this should happen after we've confirmed the rebase has succeeded
        mark_commit_reachable(&repo, extracted_commit_oid)
            .wrap_err("Marking commit as reachable for GC purposes.")?;

        event_log_db.add_events(vec![Event::CommitEvent {
            timestamp: now.duration_since(UNIX_EPOCH)?.as_secs_f64(),
            event_tx_id,
            commit_oid: extracted_commit_oid,
        }])?;

        Some(extracted_commit_oid)
    };

    // push the new commits into the dag for the rebase planner
    dag.sync_from_oids(
        effects,
        &repo,
        CommitSet::empty(),
        match extracted_commit_oid {
            None => CommitSet::from(remainder_commit_oid),
            Some(extracted_commit_oid) => vec![remainder_commit_oid, extracted_commit_oid]
                .into_iter()
                .collect(),
        },
    )?;

    enum TargetState {
        /// A checked out, detached HEAD
        DetachedHead,
        /// A checked out branch
        CurrentBranch,
        /// Any other non-checked out commit
        Other,
    }

    let head_info = repo.get_head_info()?;
    let target_state = match head_info {
        ResolvedReferenceInfo {
            oid: Some(oid),
            reference_name: Some(_),
        } if oid == target_oid => TargetState::CurrentBranch,
        ResolvedReferenceInfo {
            oid: Some(oid),
            reference_name: None,
        } if oid == target_oid => TargetState::DetachedHead,
        ResolvedReferenceInfo {
            oid: _,
            reference_name: _,
        } => TargetState::Other,
    };

    #[derive(Debug)]
    struct CleanUp {
        checkout_target: Option<CheckoutTarget>,
        rewritten_oids: Vec<(NonZeroOid, MaybeZeroOid)>,
    }

    let cleanup = match (target_state, extracted_commit_oid) {
        // branch @ split commit checked out: extend branch to include extracted
        // commit; branch will stay checked out w/o any explicit checkout
        (TargetState::CurrentBranch, Some(extracted_commit_oid)) => CleanUp {
            checkout_target: None,
            rewritten_oids: vec![(target_oid, MaybeZeroOid::NonZero(extracted_commit_oid))],
        },

        // commit to split checked out as detached HEAD, don't extend any
        // branches, but explicitly check out the newly split commit
        (TargetState::DetachedHead, _) => CleanUp {
            checkout_target: Some(CheckoutTarget::Oid(remainder_commit_oid)),
            rewritten_oids: vec![(target_oid, MaybeZeroOid::NonZero(remainder_commit_oid))],
        },

        // some other commit or branch was checked out, default behavior is fine
        (TargetState::CurrentBranch, _) | (TargetState::Other, _) => CleanUp {
            checkout_target: None,
            rewritten_oids: vec![(target_oid, MaybeZeroOid::NonZero(remainder_commit_oid))],
        },
    };

    let CleanUp {
        checkout_target,
        rewritten_oids,
    } = cleanup;

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
    let children = dag.query_children(CommitSet::from(target_oid))?;
    for child in dag.commit_set_to_vec(&children)? {
        match extracted_commit_oid {
            None => builder.move_subtree(child, vec![remainder_commit_oid])?,
            Some(extracted_commit_oid) => {
                builder.move_subtree(child, vec![extracted_commit_oid])?
            }
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
