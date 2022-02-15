//! Wrap a user-provided Git command, so that `git-branchless` can do special
//! processing.

use std::convert::TryInto;
use std::process::Command;
use std::time::SystemTime;

use eyre::Context;
use itertools::Itertools;

use crate::core::eventlog::{EventLogDb, EventTransactionId, BRANCHLESS_TRANSACTION_ID_ENV_VAR};
use crate::git::{GitRunInfo, Repo};

fn pass_through_git_command_inner(
    git_run_info: &GitRunInfo,
    args: &[&str],
    event_tx_id: Option<EventTransactionId>,
) -> eyre::Result<isize> {
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
    if let Some(event_tx_id) = event_tx_id {
        command.env(BRANCHLESS_TRANSACTION_ID_ENV_VAR, event_tx_id.to_string());
    }
    let exit_status = command.status().wrap_err("Running Git command")?;
    let exit_code = exit_status.code().unwrap_or(1).try_into()?;
    Ok(exit_code)
}

fn pass_through_git_command<S: AsRef<str> + std::fmt::Debug>(
    git_run_info: &GitRunInfo,
    args: &[S],
    event_tx_id: Option<EventTransactionId>,
) -> eyre::Result<isize> {
    pass_through_git_command_inner(
        git_run_info,
        args.iter().map(AsRef::as_ref).collect_vec().as_slice(),
        event_tx_id,
    )
}

fn make_event_tx_id<S: AsRef<str> + std::fmt::Debug>(
    args: &[S],
) -> eyre::Result<EventTransactionId> {
    let now = SystemTime::now();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = {
        let message = args.first().map(|s| s.as_ref()).unwrap_or("wrap");
        event_log_db.make_transaction_id(now, message)?
    };
    Ok(event_tx_id)
}

/// Run the provided Git command, but wrapped in an event transaction.
pub fn wrap<S: AsRef<str> + std::fmt::Debug>(
    git_run_info: &GitRunInfo,
    args: &[S],
) -> eyre::Result<isize> {
    // We may not be able to make an event transaction ID (such as if there is
    // no repository in the current directory). Ignore the error in that case.
    let event_tx_id = make_event_tx_id(args).ok();

    let exit_code = pass_through_git_command(git_run_info, args, event_tx_id)?;
    Ok(exit_code)
}
