//! Commit changes in the working copy.

#![warn(missing_docs)]
#![warn(clippy::all, clippy::as_conversions, clippy::clone_on_ref_ptr)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt::Write;
use std::io;
use std::time::SystemTime;

use cursive::backends::crossterm;
use cursive::CursiveRunnable;
use cursive_buffered_backend::BufferedBackend;

use eden_dag::DagAlgorithm;
use git_record::Recorder;
use git_record::{RecordError, RecordState};
use itertools::Itertools;
use lib::core::check_out::{check_out_commit, CheckOutCommitOptions};
use lib::core::config::get_restack_preserve_timestamps;
use lib::core::dag::{commit_set_to_vec, CommitSet, Dag};
use lib::core::effects::{Effects, OperationType};
use lib::core::eventlog::{EventLogDb, EventReplayer, EventTransactionId};
use lib::core::formatting::Pluralize;
use lib::core::repo_ext::RepoExt;
use lib::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanError, BuildRebasePlanOptions, ExecuteRebasePlanOptions,
    ExecuteRebasePlanResult, MergeConflictRemediation, RebasePlanBuilder, RebasePlanPermissions,
    RepoResource,
};
use lib::git::{
    process_diff_for_record, update_index, CategorizedReferenceName, FileMode, GitRunInfo,
    NonZeroOid, Repo, ResolvedReferenceInfo, Stage, UpdateIndexCommand, WorkingCopyChangesType,
    WorkingCopySnapshot,
};
use lib::util::ExitCode;
use rayon::ThreadPoolBuilder;

/// Commit changes in the working copy.
pub fn record(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    message: Option<String>,
    interactive: bool,
    branch_name: Option<String>,
    detach: bool,
    insert: bool,
) -> eyre::Result<ExitCode> {
    let now = SystemTime::now();
    let repo = Repo::from_dir(&git_run_info.working_directory)?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "record")?;

    let (snapshot, working_copy_changes_type) = {
        let head_info = repo.get_head_info()?;
        let index = repo.get_index()?;
        let (snapshot, _status) =
            repo.get_status(effects, git_run_info, &index, &head_info, Some(event_tx_id))?;

        let working_copy_changes_type = snapshot.get_working_copy_changes_type()?;
        match working_copy_changes_type {
            WorkingCopyChangesType::None => {
                writeln!(
                    effects.get_output_stream(),
                    "There are no changes to tracked files in the working copy to commit."
                )?;
                return Ok(ExitCode(0));
            }
            WorkingCopyChangesType::Unstaged | WorkingCopyChangesType::Staged => {}
            WorkingCopyChangesType::Conflicts => {
                writeln!(
                    effects.get_output_stream(),
                    "Cannot commit changes while there are unresolved merge conflicts."
                )?;
                writeln!(
                    effects.get_output_stream(),
                    "Resolve them and try again. Aborting."
                )?;
                return Ok(ExitCode(1));
            }
        }
        (snapshot, working_copy_changes_type)
    };

    if let Some(branch_name) = branch_name {
        let exit_code = check_out_commit(
            effects,
            git_run_info,
            &repo,
            &event_log_db,
            event_tx_id,
            None,
            &CheckOutCommitOptions {
                additional_args: vec![OsString::from("-b"), OsString::from(branch_name)],
                reset: false,
                render_smartlog: false,
            },
        )?;
        if !exit_code.is_success() {
            return Ok(exit_code);
        }
    }

    let commit_exit_code = if interactive {
        if working_copy_changes_type == WorkingCopyChangesType::Staged {
            writeln!(
                effects.get_output_stream(),
                "Cannot select changes interactively while there are already staged changes."
            )?;
            writeln!(
                effects.get_output_stream(),
                "Either commit or unstage your changes and try again. Aborting."
            )?;
            ExitCode(1)
        } else {
            record_interactive(
                effects,
                git_run_info,
                &repo,
                &snapshot,
                event_tx_id,
                message.as_deref(),
            )?
        }
    } else {
        let args = {
            let mut args = vec!["commit"];
            if let Some(message) = &message {
                args.extend(["--message", message]);
            }
            if working_copy_changes_type == WorkingCopyChangesType::Unstaged {
                args.push("--all");
            }
            args
        };
        git_run_info.run_direct_no_wrapping(Some(event_tx_id), &args)?
    };
    if !commit_exit_code.is_success() {
        return Ok(commit_exit_code);
    }

    if detach {
        let head_info = repo.get_head_info()?;
        if let ResolvedReferenceInfo {
            oid: Some(oid),
            reference_name: Some(reference_name),
        } = &head_info
        {
            let head_commit = repo.find_commit_or_fail(*oid)?;
            return match head_commit.get_parents().as_slice() {
                [] => git_run_info.run(
                    effects,
                    Some(event_tx_id),
                    &[
                        "update-ref",
                        "-d",
                        reference_name.as_str(),
                        &oid.to_string(),
                    ],
                ),
                [parent_commit] => {
                    let branch_name =
                        CategorizedReferenceName::new(reference_name).remove_prefix()?;
                    repo.detach_head(&head_info)?;
                    git_run_info.run(
                        effects,
                        Some(event_tx_id),
                        &[
                            "branch",
                            "-f",
                            &branch_name,
                            &parent_commit.get_oid().to_string(),
                        ],
                    )
                }
                parent_commits => {
                    eyre::bail!("git-branchless record --detach called on a merge commit, but it should only be capable of creating zero- or one-parent commits. Parents: {parent_commits:?}");
                }
            };
        }
    }

    if insert {
        let exit_code = insert_before_siblings(effects, git_run_info, now, event_tx_id)?;
        if !exit_code.is_success() {
            return Ok(exit_code);
        }
    }

    Ok(ExitCode(0))
}

fn record_interactive(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    snapshot: &WorkingCopySnapshot,
    event_tx_id: EventTransactionId,
    message: Option<&str>,
) -> eyre::Result<ExitCode> {
    let file_states = {
        let (effects, _progress) = effects.start_operation(OperationType::CalculateDiff);
        let old_tree = snapshot.commit_stage0.get_tree()?;
        let new_tree = snapshot.commit_unstaged.get_tree()?;
        let diff = repo.get_diff_between_trees(
            &effects,
            Some(&old_tree),
            &new_tree,
            // We manually add context to the git-record output, so suppress the context lines here.
            0,
        )?;
        process_diff_for_record(repo, &diff)?
    };
    let record_state = RecordState { file_states };

    let siv = CursiveRunnable::new(|| -> io::Result<_> {
        // Use crossterm to ensure that we support Windows.
        let crossterm_backend = crossterm::Backend::init()?;
        Ok(Box::new(BufferedBackend::new(crossterm_backend)))
    });
    let siv = siv.into_runner();

    let recorder = Recorder::new(record_state);
    let result = recorder.run(siv);
    let RecordState {
        file_states: result,
    } = match result {
        Ok(result) => result,
        Err(RecordError::Cancelled) => {
            println!("Aborted.");
            return Ok(ExitCode(1));
        }
    };

    let update_index_script: Vec<UpdateIndexCommand> = result
        .into_iter()
        .map(|(path, file_state)| -> eyre::Result<UpdateIndexCommand> {
            let (selected, _unselected) = file_state.get_selected_contents();
            let oid = repo.create_blob_from_contents(selected.as_bytes())?;
            let command = UpdateIndexCommand::Update {
                path,
                stage: Stage::Stage0,
                // TODO: use `FileMode::BlobExecutable` when appropriate.
                mode: FileMode::Blob,
                oid,
            };
            Ok(command)
        })
        .try_collect()?;
    let index = repo.get_index()?;
    update_index(
        git_run_info,
        repo,
        &index,
        event_tx_id,
        &update_index_script,
    )?;

    let args = {
        let mut args = vec!["commit"];
        if let Some(message) = message {
            args.extend(["--message", message]);
        }
        args
    };
    git_run_info.run_direct_no_wrapping(Some(event_tx_id), &args)
}

fn insert_before_siblings(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    now: SystemTime,
    event_tx_id: EventTransactionId,
) -> eyre::Result<ExitCode> {
    // Reopen the repository since references may have changed.
    let repo = Repo::from_dir(&git_run_info.working_directory)?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let references_snapshot = repo.get_references_snapshot()?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let head_info = repo.get_head_info()?;
    let head_oid = match head_info {
        ResolvedReferenceInfo {
            oid: Some(head_oid),
            reference_name: _,
        } => head_oid,
        ResolvedReferenceInfo {
            oid: None,
            reference_name: _,
        } => {
            return Ok(ExitCode(0));
        }
    };

    let dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;
    let head_commit = repo.find_commit_or_fail(head_oid)?;
    let head_commit_set = CommitSet::from(head_oid);
    let parents = dag.query().parents(head_commit_set.clone())?;
    let children = dag.query().children(parents)?;
    let siblings = children.difference(&head_commit_set);
    let build_options = BuildRebasePlanOptions {
        force_rewrite_public_commits: false,
        dump_rebase_constraints: false,
        dump_rebase_plan: false,
        detect_duplicate_commits_via_patch_id: true,
    };

    let rebase_plan_result =
        match RebasePlanPermissions::verify_rewrite_set(&dag, build_options, &siblings)? {
            Err(err) => Err(err),
            Ok(permissions) => {
                let head_commit_parents: HashSet<_> =
                    head_commit.get_parent_oids().into_iter().collect();
                let mut builder = RebasePlanBuilder::new(&dag, permissions);
                for sibling_oid in commit_set_to_vec(&siblings)? {
                    let sibling_commit = repo.find_commit_or_fail(sibling_oid)?;
                    let parent_oids = sibling_commit.get_parent_oids();
                    let new_parent_oids = parent_oids
                        .into_iter()
                        .map(|parent_oid| {
                            if head_commit_parents.contains(&parent_oid) {
                                head_oid
                            } else {
                                parent_oid
                            }
                        })
                        .collect_vec();
                    builder.move_subtree(sibling_oid, new_parent_oids)?;
                }
                let thread_pool = ThreadPoolBuilder::new().build()?;
                let repo_pool = RepoResource::new_pool(&repo)?;
                builder.build(effects, &thread_pool, &repo_pool)?
            }
        };

    let rebase_plan = match rebase_plan_result {
        Ok(Some(rebase_plan)) => rebase_plan,

        Ok(None) => {
            // Nothing to do, since there were no siblings to move.
            return Ok(ExitCode(0));
        }

        Err(BuildRebasePlanError::ConstraintCycle { .. }) => {
            writeln!(
                effects.get_output_stream(),
                "BUG: constraint cycle detected when moving siblings, which shouldn't be possible."
            )?;
            return Ok(ExitCode(1));
        }

        Err(err @ BuildRebasePlanError::MoveIllegalCommits { .. }) => {
            err.describe(effects, &repo)?;
            return Ok(ExitCode(1));
        }

        Err(BuildRebasePlanError::MovePublicCommits {
            public_commits_to_move,
        }) => {
            let example_bad_commit_oid = public_commits_to_move
                .first()?
                .ok_or_else(|| eyre::eyre!("BUG: could not get OID of a public commit to move"))?;
            let example_bad_commit_oid = NonZeroOid::try_from(example_bad_commit_oid)?;
            let example_bad_commit = repo.find_commit_or_fail(example_bad_commit_oid)?;
            writeln!(
                effects.get_output_stream(),
                "\
You are trying to rewrite {}, such as: {}
It is generally not advised to rewrite public commits, because your
collaborators will have difficulty merging your changes.
To proceed anyways, run: git move -f -s 'siblings(.)",
                Pluralize {
                    determiner: None,
                    amount: public_commits_to_move.count()?,
                    unit: ("public commit", "public commits")
                },
                effects
                    .get_glyphs()
                    .render(example_bad_commit.friendly_describe(effects.get_glyphs())?)?,
            )?;
            return Ok(ExitCode(0));
        }
    };

    let execute_options = ExecuteRebasePlanOptions {
        now,
        event_tx_id,
        preserve_timestamps: get_restack_preserve_timestamps(&repo)?,
        force_in_memory: true,
        force_on_disk: false,
        resolve_merge_conflicts: false,
        check_out_commit_options: Default::default(),
    };
    let result = execute_rebase_plan(
        effects,
        git_run_info,
        &repo,
        &event_log_db,
        &rebase_plan,
        &execute_options,
    )?;
    match result {
        ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ } => Ok(ExitCode(0)),
        ExecuteRebasePlanResult::DeclinedToMerge { failed_merge_info } => {
            failed_merge_info.describe(effects, &repo, MergeConflictRemediation::Insert)?;
            Ok(ExitCode(0))
        }
        ExecuteRebasePlanResult::Failed { exit_code } => Ok(exit_code),
    }
}
