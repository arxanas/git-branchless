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

#[derive(Debug, PartialEq)]
/// What should `split` do with the extracted changes?
pub enum SplitMode {
    DetachAfter,
    Discard,
    InsertAfter,
    InsertBefore,
}

/// Split a commit and restack its descendants.
#[instrument]
pub fn split(
    effects: &Effects,
    revset: Revset,
    resolve_revset_options: &ResolveRevsetOptions,
    files_to_extract: Vec<String>,
    split_mode: SplitMode,
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

    //
    // a-t-b
    //
    // a-r-x-b (default)
    // a-x-r-b (before)
    // a-r-b   (detach)
    //   \-x
    // a-r-b   (discard)
    //
    // default: x == t tree, x is t with changes removed
    // before:  r == t tree, e is a with changes added
    // detach:  (same as default, different rebase)
    // discard: (same as default, w/o any rebase)
    //
    // below:
    // a => parent
    // t => target
    // r => remainder
    // x => extracted

    let target_commit = repo.find_commit_or_fail(target_oid)?;
    let target_tree = target_commit.get_tree()?;
    let parent_commits = target_commit.get_parents();
    let (parent_tree, mut remainder_tree) = match (&split_mode, parent_commits.as_slice()) {
        // split the commit by removing the changes from the target, and then
        // cherry picking the orignal target as the "extracted" commit
        (SplitMode::InsertAfter, [only_parent])
        | (SplitMode::Discard, [only_parent])
        | (SplitMode::DetachAfter, [only_parent]) => {
            (only_parent.get_tree()?, target_commit.get_tree()?)
        }

        // split the commit by adding the changed to a copy of the parent tree,
        // then rebasing the orignal target onto the extracted commit
        (SplitMode::InsertBefore, [only_parent]) => {
            (only_parent.get_tree()?, only_parent.get_tree()?)
        }

        // no parent: use an empty tree for comparison
        (SplitMode::InsertAfter, []) | (SplitMode::Discard, []) | (SplitMode::DetachAfter, []) => {
            (make_empty_tree(&repo)?, target_commit.get_tree()?)
        }

        // no parent: add extracted changes to an empty tree
        (SplitMode::InsertBefore, []) => (make_empty_tree(&repo)?, make_empty_tree(&repo)?),

        (_, [..]) => {
            writeln!(
                effects.get_error_stream(),
                "Cannot split merge commit {}.",
                target_oid
            )?;
            return Ok(Err(ExitCode(1)));
        }
    };

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
        let temp_tree_oid = match (parent_entry, target_entry, &split_mode) {
            // added/modified & InsertBefore => add to extracted commit
            (None, Some(commit_entry), SplitMode::InsertBefore)
            | (Some(_), Some(commit_entry), SplitMode::InsertBefore) => {
                remainder_tree.add_or_replace(&repo, path, &commit_entry)?
            }

            // removed & InsertBefore => remove from remainder commit
            (Some(_), None, SplitMode::InsertBefore) => remainder_tree.remove(&repo, path)?,

            // added => remove from remainder commit
            (None, Some(_), SplitMode::InsertAfter)
            | (None, Some(_), SplitMode::DetachAfter)
            | (None, Some(_), SplitMode::Discard) => remainder_tree.remove(&repo, path)?,

            // deleted/modified => replace w/ parent content in split commit
            (Some(parent_entry), _, _) => {
                remainder_tree.add_or_replace(&repo, path, &parent_entry)?
            }

            (None, _, _) => {
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
        let (effects, _progress) =
            effects.start_operation(lib::core::effects::OperationType::CalculateDiff);
        let (old_tree, new_tree) = if let SplitMode::InsertBefore = &split_mode {
            (&parent_tree, &remainder_tree)
        } else {
            (&remainder_tree, &target_tree)
        };
        let diff = repo.get_diff_between_trees(
            &effects,
            Some(old_tree),
            new_tree,
            0, // we don't care about the context here
        )?;

        summarize_diff_for_temporary_commit(&diff)?
    };

    // before => split commit is created on parent as "extracted", target is rebased onto split
    // after => target is amended as "split", split is cherry picked onto split as "extracted"

    // FIXME terminology if wrong here: remainder is correct for "After", but
    // this is the "extracted" commit for InsertBefore
    let remainder_commit_oid = if let SplitMode::InsertBefore = split_mode {
        repo.create_commit(
            None,
            &target_commit.get_author(),
            &target_commit.get_committer(),
            format!("temp(split): {message}").as_str(),
            &remainder_tree,
            parent_commits.iter().collect(),
        )?
    } else {
        target_commit.amend_commit(None, None, None, None, Some(&remainder_tree))?
    };
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

    // FIXME terminology if wrong here: extracted is correct for "After" and
    // Discard, but the extracted commit is not None for InsertBefore, it's just
    // handled in a different way
    let extracted_commit_oid = match split_mode {
        SplitMode::InsertBefore | SplitMode::Discard => None,
        SplitMode::InsertAfter | SplitMode::DetachAfter => {
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
                if let SplitMode::InsertBefore = &split_mode {
                    parent_commits.iter().collect()
                } else {
                    vec![&remainder_commit]
                },
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
        }
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

    let head_info = repo.get_head_info()?;
    let (checkout_target, rewritten_oids, rebase_force_detach) = match (head_info, &split_mode) {
        // branch @ target commit checked out: extend branch to include extracted
        // commit; branch will stay checked out w/o any explicit checkout        ResolvedReferenceInfo {
        (
            ResolvedReferenceInfo {
                oid: Some(oid),
                reference_name: Some(_),
            },
            // not DetatchAfter
            SplitMode::InsertAfter | SplitMode::Discard,
        ) if oid == target_oid && extracted_commit_oid.is_some() => (
            None,
            vec![(
                target_oid,
                MaybeZeroOid::NonZero(extracted_commit_oid.unwrap()),
            )],
            false,
        ),

        // same as above, but InsertBefore; do not move branches
        (
            ResolvedReferenceInfo {
                oid: Some(oid),
                reference_name: Some(_),
            },
            SplitMode::InsertBefore,
        ) if oid == target_oid => (None, vec![], false),

        // target checked out as detached HEAD, don't extend any
        // branches, but explicitly check out the newly split commit
        (
            ResolvedReferenceInfo {
                oid: Some(oid),
                reference_name: None,
            },
            SplitMode::InsertAfter | SplitMode::Discard | SplitMode::DetachAfter,
        ) if oid == target_oid => (
            Some(CheckoutTarget::Oid(remainder_commit_oid)),
            vec![(target_oid, MaybeZeroOid::NonZero(remainder_commit_oid))],
            false,
        ),

        // same as above, but InsertBefore; do not move branches
        (
            ResolvedReferenceInfo {
                oid: Some(oid),
                reference_name: None,
            },
            SplitMode::InsertBefore,
        ) if oid == target_oid => (None, vec![], true),

        // some other commit or branch was checked out, default behavior is fine
        (
            ResolvedReferenceInfo {
                oid: _,
                reference_name: _,
            },
            _,
        ) => (
            None,
            vec![(target_oid, MaybeZeroOid::NonZero(remainder_commit_oid))],
            false,
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
    if let SplitMode::InsertBefore = &split_mode {
        builder.move_subtree(target_oid, vec![remainder_commit_oid])?
    } else {
        let children = dag.query_children(CommitSet::from(target_oid))?;
        for child in dag.commit_set_to_vec(&children)? {
            match (&split_mode, extracted_commit_oid) {
                (_, None) | (SplitMode::DetachAfter, Some(_)) => {
                    builder.move_subtree(child, vec![remainder_commit_oid])?
                }
                (_, Some(extracted_commit_oid)) => {
                    builder.move_subtree(child, vec![extracted_commit_oid])?
                }
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
                    force_detach: rebase_force_detach,
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
