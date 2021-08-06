//! Wrap a user-provided Git command, so that `git-branchless` can do special
//! processing.

use std::convert::TryInto;
use std::process::Command;
use std::time::SystemTime;

use eyre::Context;

use crate::core::eventlog::{EventLogDb, EventTransactionId, BRANCHLESS_TRANSACTION_ID_ENV_VAR};
use crate::git::{GitRunInfo, Repo};

fn pass_through_git_command<S: AsRef<str> + std::fmt::Debug>(
    git_run_info: &GitRunInfo,
    args: &[S],
    event_tx_id: Option<EventTransactionId>,
) -> eyre::Result<isize> {
    let GitRunInfo {
        path_to_git,
        working_directory,
        env,
    } = git_run_info;
    let mut command = Command::new(path_to_git);
    command.current_dir(working_directory);
    command.args(args.iter().map(|arg| arg.as_ref()));
    command.env_clear();
    command.envs(env.iter());
    if let Some(event_tx_id) = event_tx_id {
        command.env(BRANCHLESS_TRANSACTION_ID_ENV_VAR, event_tx_id.to_string());
    }
    let exit_status = command
        .status()
        .wrap_err_with(|| format!("Running program: {:?} {:?}", path_to_git, args))?;
    let exit_code = exit_status.code().unwrap_or(1).try_into()?;
    Ok(exit_code)
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

#[cfg(test)]
mod tests {
    use crate::core::eventlog::testing::{get_event_replayer_events, redact_event_timestamp};
    use crate::core::eventlog::{Event, EventLogDb, EventReplayer};
    use crate::testing::make_git;

    #[test]
    fn test_wrap_rebase_in_transaction() -> eyre::Result<()> {
        let git = make_git()?;

        if !git.supports_reference_transactions()? {
            return Ok(());
        }

        git.init_repo()?;
        git.run(&["checkout", "-b", "foo"])?;
        git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        git.run(&["checkout", "master"])?;

        git.run(&["branchless", "wrap", "rebase", "foo"])?;

        let repo = git.get_repo()?;
        let conn = repo.get_db_conn()?;
        let event_log_db = EventLogDb::new(&conn)?;
        let event_replayer = EventReplayer::from_event_log_db(&repo, &event_log_db)?;
        let events: Vec<Event> = get_event_replayer_events(&event_replayer)
            .iter()
            .map(|event| redact_event_timestamp(event.clone()))
            .collect();

        insta::assert_debug_snapshot!(events, @r###"
        [
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    1,
                ),
                ref_name: "refs/heads/foo",
                old_oid: 0000000000000000000000000000000000000000,
                new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    2,
                ),
                ref_name: "HEAD",
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    3,
                ),
                ref_name: "HEAD",
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    3,
                ),
                ref_name: "refs/heads/foo",
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                message: None,
            },
            CommitEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    4,
                ),
                commit_oid: NonZeroOid {
                    inner: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                },
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    5,
                ),
                ref_name: "HEAD",
                old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    5,
                ),
                ref_name: "refs/heads/foo",
                old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
            CommitEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    6,
                ),
                commit_oid: NonZeroOid {
                    inner: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                },
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    7,
                ),
                ref_name: "HEAD",
                old_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    8,
                ),
                ref_name: "HEAD",
                old_oid: 0000000000000000000000000000000000000000,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    8,
                ),
                ref_name: "HEAD",
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    8,
                ),
                ref_name: "refs/heads/master",
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
        ]
        "###);

        Ok(())
    }

    #[test]
    fn test_wrap_explicit_git_executable() -> eyre::Result<()> {
        let git = make_git()?;

        git.init_repo()?;
        let (stdout, _stderr) = git.run(&[
            "branchless",
            "wrap",
            "--git-executable",
            // Don't use a hardcoded executable like `echo` here (see
            // https://github.com/arxanas/git-branchless/issues/26). We also
            // don't want to use `git`, since that's the default value for
            // this argument, so we wouldn't be able to tell if it was
            // working. But we're certain to have `git-branchless` on
            // `PATH`!
            "git-branchless",
            "--",
            "--help",
        ])?;
        assert!(stdout.contains("Branchless workflow for Git."));
        Ok(())
    }
}
