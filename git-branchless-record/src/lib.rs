//! Commit changes in the working copy.

#![warn(missing_docs)]
#![warn(clippy::all, clippy::as_conversions, clippy::clone_on_ref_ptr)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

use std::fmt::Write;
use std::io;
use std::time::SystemTime;

use cursive::backends::crossterm;
use cursive::CursiveRunnable;
use cursive_buffered_backend::BufferedBackend;

use git_record::Recorder;
use git_record::{RecordError, RecordState};
use itertools::Itertools;
use lib::core::effects::{Effects, OperationType};
use lib::core::eventlog::{EventLogDb, EventTransactionId};
use lib::git::{
    process_diff_for_record, update_index, CategorizedReferenceName, FileMode, GitRunInfo, Repo,
    ResolvedReferenceInfo, Stage, UpdateIndexCommand, WorkingCopyChangesType, WorkingCopySnapshot,
};
use lib::util::ExitCode;

/// Commit changes in the working copy.
pub fn record(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    message: Option<String>,
    interactive: bool,
    detach: bool,
) -> eyre::Result<ExitCode> {
    let repo = Repo::from_dir(&git_run_info.working_directory)?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(SystemTime::now(), "record")?;

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
