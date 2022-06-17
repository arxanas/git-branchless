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
use lib::core::eventlog::EventLogDb;
use lib::git::{
    process_diff_for_record, update_index, FileMode, GitRunInfo, Repo, Stage, UpdateIndexCommand,
};
use lib::util::ExitCode;

pub fn record(effects: &Effects, git_run_info: &GitRunInfo) -> eyre::Result<ExitCode> {
    let repo = Repo::from_dir(&git_run_info.working_directory)?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(SystemTime::now(), "record")?;

    let files = {
        let head_info = repo.get_head_info()?;
        let index = repo.get_index()?;
        let (snapshot, _status) =
            repo.get_status(effects, git_run_info, &index, &head_info, Some(event_tx_id))?;

        if snapshot.has_conflicts()? {
            writeln!(
                effects.get_output_stream(),
                "Cannot select changes while there are unresolved merge conflicts."
            )?;
            writeln!(
                effects.get_output_stream(),
                "Resolve them and try again. Aborting."
            )?;
            return Ok(ExitCode(1));
        }

        let (effects, _progress) = effects.start_operation(OperationType::CalculateDiff);
        let old_tree = snapshot.commit_stage0.get_tree()?;
        let new_tree = snapshot.commit_unstaged.get_tree()?;
        let diff = repo.get_diff_between_trees(&effects, Some(&old_tree), &new_tree, 3)?;
        process_diff_for_record(&repo, &diff)?
    };
    let record_state = if files.is_empty() {
        println!("There are no unstaged changes to select.");
        return Ok(ExitCode(0));
    } else {
        RecordState { files }
    };

    let siv = CursiveRunnable::new(|| -> io::Result<_> {
        // Use crossterm to ensure that we support Windows.
        let crossterm_backend = crossterm::Backend::init()?;
        Ok(Box::new(BufferedBackend::new(crossterm_backend)))
    });
    let siv = siv.into_runner();

    let recorder = Recorder::new(record_state);
    let result = recorder.run(siv);
    let RecordState { files: result } = match result {
        Ok(result) => result,
        Err(RecordError::Cancelled) => {
            println!("Aborted.");
            return Ok(ExitCode(1));
        }
    };

    let update_index_script: Vec<UpdateIndexCommand> = result
        .into_iter()
        .map(|(path, file_content)| -> eyre::Result<UpdateIndexCommand> {
            let (selected, _unselected) = file_content.get_selected_contents();
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
        &repo,
        &index,
        event_tx_id,
        &update_index_script,
    )?;

    git_run_info.run_direct_no_wrapping(Some(event_tx_id), &["commit"])
}
