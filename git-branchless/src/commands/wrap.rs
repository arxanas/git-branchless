//! Wrap a user-provided Git command, so that `git-branchless` can do special
//! processing.

use std::process::Command;
use std::time::SystemTime;

use eyre::Context;
use itertools::Itertools;

use lib::core::eventlog::{
    Event, EventLogDb, EventTransactionId, BRANCHLESS_TRANSACTION_ID_ENV_VAR,
};
use lib::core::repo_ext::{RepoExt, RepoReferencesSnapshot};
use lib::git::{GitRunInfo, MaybeZeroOid, Repo};
use lib::util::{ExitCode, EyreExitOr};

struct RepoState {
    repo: Repo,
    references_snapshot: RepoReferencesSnapshot,
    event_tx_id: EventTransactionId,
}

fn pass_through_git_command_inner(
    git_run_info: &GitRunInfo,
    args: &[&str],
    repo_state: Option<&RepoState>,
) -> EyreExitOr<()> {
    let GitRunInfo {
        path_to_git,
        working_directory,
        env,
    } = git_run_info;
    let mut command = Command::new(path_to_git);
    command.current_dir(working_directory);
    command.args(args);
    command.env_clear();
    command.envs(env.iter());
    if let Some(RepoState {
        repo: _,
        references_snapshot: _,
        event_tx_id,
    }) = repo_state
    {
        command.env(BRANCHLESS_TRANSACTION_ID_ENV_VAR, event_tx_id.to_string());
    }
    let exit_status = command.status().wrap_err("Running Git command")?;
    let exit_code: isize = exit_status.code().unwrap_or(1).try_into()?;
    let exit_code = ExitCode(exit_code);
    if exit_code.is_success() {
        Ok(Ok(()))
    } else {
        Ok(Err(exit_code))
    }
}

fn pass_through_git_command<S: AsRef<str> + std::fmt::Debug>(
    git_run_info: &GitRunInfo,
    args: &[S],
    repo_state: Option<&RepoState>,
) -> EyreExitOr<()> {
    pass_through_git_command_inner(
        git_run_info,
        args.iter().map(AsRef::as_ref).collect_vec().as_slice(),
        repo_state,
    )
}

fn get_repo_state<S: AsRef<str> + std::fmt::Debug>(args: &[S]) -> eyre::Result<RepoState> {
    let now = SystemTime::now();
    let repo = Repo::from_current_dir()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = {
        let message = args.first().map(|s| s.as_ref()).unwrap_or("wrap");
        event_log_db.make_transaction_id(now, message)?
    };
    Ok(RepoState {
        repo,
        references_snapshot,
        event_tx_id,
    })
}

/// @nocommit: explain that this is a hack
fn record_reference_diff(repo_state: &RepoState) -> eyre::Result<()> {
    // @nocommit: do we need to reopen the repo?
    // let repo = Repo::from_current_dir()?;
    let references_snapshot = repo_state.repo.get_references_snapshot()?;
    let now = SystemTime::now();
    let conn = repo_state.repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;

    let references_diff = repo_state.references_snapshot.diff(&references_snapshot);
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();
    let events: Vec<Event> = references_diff
        .into_iter()
        .map(
            |(reference_name, old_info, new_info)| Event::RefUpdateEvent {
                timestamp,
                event_tx_id: repo_state.event_tx_id,
                ref_name: reference_name.clone(),
                old_oid: MaybeZeroOid::from(old_info.oid),
                new_oid: MaybeZeroOid::from(new_info.oid),
                message: None,
            },
        )
        .collect();
    event_log_db.add_events(events)?;
    Ok(())
}

/// Run the provided Git command, but wrapped in an event transaction.
pub fn wrap<S: AsRef<str> + std::fmt::Debug>(
    git_run_info: &GitRunInfo,
    args: &[S],
) -> EyreExitOr<()> {
    // We may not be able to make an event transaction ID (such as if there is
    // no repository in the current directory). Ignore the error in that case.
    let repo_state = get_repo_state(args).ok();

    let exit_code = pass_through_git_command(git_run_info, args, repo_state.as_ref())?;
    if let Some(repo_state) = repo_state {
        record_reference_diff(&repo_state)?;
    }
    Ok(exit_code)
}
