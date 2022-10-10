//! Manage working copy snapshots. These commands are primarily intended for
//! testing and debugging.

use std::convert::TryInto;
use std::fmt::Write;
use std::time::SystemTime;

use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;
use eyre::Context;
use lib::core::check_out::{create_snapshot, restore_snapshot};
use lib::core::effects::Effects;
use lib::core::eventlog::EventLogDb;
use lib::git::{GitRunInfo, GitRunResult, NonZeroOid, Repo, WorkingCopySnapshot};
use lib::util::ExitCode;

pub fn create(effects: &Effects, git_run_info: &GitRunInfo) -> eyre::Result<ExitCode> {
    let repo = Repo::from_dir(&git_run_info.working_directory)?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(SystemTime::now(), "snapshot create")?;
    let snapshot = create_snapshot(effects, git_run_info, &repo, &event_log_db, event_tx_id)?;
    writeln!(
        effects.get_output_stream(),
        "{}",
        snapshot.base_commit.get_oid()
    )?;

    // Don't write `git reset` output to stdout.
    let GitRunResult {
        exit_code,
        stdout: _,
        stderr: _,
    } = git_run_info
        .run_silent(
            &repo,
            Some(event_tx_id),
            &["reset", "--hard", "HEAD", "--"],
            Default::default(),
        )
        .wrap_err("Discarding working copy")?;

    if exit_code != 0 {
        writeln!(
            effects.get_output_stream(),
            "{}",
            effects.get_glyphs().render(StyledString::styled(
                "Failed to clean up working copy state".to_string(),
                BaseColor::Red.light()
            ))?
        )?;
        return Ok(ExitCode(exit_code.try_into()?));
    }

    Ok(ExitCode(0))
}

pub fn restore(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    snapshot_oid: NonZeroOid,
) -> eyre::Result<ExitCode> {
    let repo = Repo::from_dir(&git_run_info.working_directory)?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(SystemTime::now(), "snapshot restore")?;

    let base_commit = repo.find_commit_or_fail(snapshot_oid)?;
    let snapshot = match WorkingCopySnapshot::try_from_base_commit(&repo, &base_commit)? {
        Some(snapshot) => snapshot,
        None => {
            writeln!(
                effects.get_error_stream(),
                "Not a snapshot commit: {}",
                snapshot_oid
            )?;
            return Ok(ExitCode(1));
        }
    };

    restore_snapshot(effects, git_run_info, &repo, event_tx_id, &snapshot)
}
