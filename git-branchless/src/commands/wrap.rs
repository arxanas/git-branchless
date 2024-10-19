//! Wrap a user-provided Git command, so that `git-branchless` can do special
//! processing.

use std::process::Command;
use std::time::SystemTime;

use eyre::Context;

use lib::core::check_out::record_reference_diff;
use lib::core::config::get_track_ref_updates;
use lib::core::effects::Effects;
use lib::core::eventlog::{
    EventCursor, EventLogDb, EventReplayer, EventTransactionId, BRANCHLESS_TRANSACTION_ID_ENV_VAR,
};
use lib::core::repo_ext::{RepoExt, RepoReferencesSnapshot};
use lib::git::{GitRunInfo, Repo};
use lib::util::{ExitCode, EyreExitOr};

struct RepoState {
    repo: Repo,
    references_snapshot: RepoReferencesSnapshot,
    event_tx_id: EventTransactionId,
    event_cursor: EventCursor,
}

fn pass_through_git_command(
    git_run_info: &GitRunInfo,
    args: &[String],
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
        event_cursor,
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

fn get_repo_state(effects: &Effects, args: &[String]) -> eyre::Result<RepoState> {
    let now = SystemTime::now();
    let repo = Repo::from_current_dir()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    // FIXME: Most commands will also construct an `EventReplayer`, so there's
    // an unnecessary additional O(n) pass here to get the event cursor.
    // However, it should be straightforward to determine the event cursor
    // without reading the entire database contents (just count the number of
    // rows).
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let event_tx_id = {
        let message = args.first().map(|s| s.as_ref()).unwrap_or("wrap");
        event_log_db.make_transaction_id(now, message)?
    };
    Ok(RepoState {
        repo,
        references_snapshot,
        event_tx_id,
        event_cursor,
    })
}

/// Run the provided Git command, but wrapped in an event transaction.
pub fn wrap(effects: &Effects, git_run_info: &GitRunInfo, args: &[String]) -> EyreExitOr<()> {
    // We may not be able to make an event transaction ID (such as if there is
    // no repository in the current directory). Ignore the error in that case.
    let repo_state = get_repo_state(effects, args).ok();

    let exit_code = pass_through_git_command(git_run_info, args, repo_state.as_ref())?;
    if let Some(repo_state) = repo_state {
        // @nocommit: correct condition?
        if !get_track_ref_updates(&repo_state.repo)? {
            // @nocommit
            // record_reference_diff(effects, repo_state.event_tx_id, repo_state.event_cursor)?;
        }
    }
    Ok(exit_code)
}
