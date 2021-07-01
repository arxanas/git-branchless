use branchless::core::eventlog::testing::{get_event_replayer_events, redact_event_timestamp};
use branchless::core::eventlog::{Event, EventLogDb, EventReplayer};
use branchless::testing::{make_git, GitRunOptions};

#[test]
fn test_wrap_rebase_in_transaction() -> anyhow::Result<()> {
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
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
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
                    old_ref: None,
                    new_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    message: None,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        2,
                    ),
                    ref_name: "HEAD",
                    old_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    new_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    message: None,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        3,
                    ),
                    ref_name: "HEAD",
                    old_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    new_ref: Some(
                        "62fc20d2a290daea0d52bdc2ed2ad4be6491010e",
                    ),
                    message: None,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        3,
                    ),
                    ref_name: "refs/heads/foo",
                    old_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    new_ref: Some(
                        "62fc20d2a290daea0d52bdc2ed2ad4be6491010e",
                    ),
                    message: None,
                },
                CommitEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        4,
                    ),
                    commit_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        5,
                    ),
                    ref_name: "HEAD",
                    old_ref: Some(
                        "62fc20d2a290daea0d52bdc2ed2ad4be6491010e",
                    ),
                    new_ref: Some(
                        "96d1c37a3d4363611c49f7e52186e189a04c531f",
                    ),
                    message: None,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        5,
                    ),
                    ref_name: "refs/heads/foo",
                    old_ref: Some(
                        "62fc20d2a290daea0d52bdc2ed2ad4be6491010e",
                    ),
                    new_ref: Some(
                        "96d1c37a3d4363611c49f7e52186e189a04c531f",
                    ),
                    message: None,
                },
                CommitEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        6,
                    ),
                    commit_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        7,
                    ),
                    ref_name: "HEAD",
                    old_ref: Some(
                        "96d1c37a3d4363611c49f7e52186e189a04c531f",
                    ),
                    new_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    message: None,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        8,
                    ),
                    ref_name: "HEAD",
                    old_ref: None,
                    new_ref: Some(
                        "96d1c37a3d4363611c49f7e52186e189a04c531f",
                    ),
                    message: None,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        8,
                    ),
                    ref_name: "HEAD",
                    old_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    new_ref: Some(
                        "96d1c37a3d4363611c49f7e52186e189a04c531f",
                    ),
                    message: None,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        8,
                    ),
                    ref_name: "refs/heads/master",
                    old_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    new_ref: Some(
                        "96d1c37a3d4363611c49f7e52186e189a04c531f",
                    ),
                    message: None,
                },
            ]
            "###);

    Ok(())
}

#[test]
fn test_wrap_explicit_git_executable() -> anyhow::Result<()> {
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

#[test]
fn test_wrap_without_repo() -> anyhow::Result<()> {
    let git = make_git()?;

    let (stdout, stderr) = git.run_with_options(
        &["branchless", "wrap", "status"],
        &GitRunOptions {
            expected_exit_code: 128,
            ..Default::default()
        },
    )?;
    insta::assert_snapshot!(stderr, @"fatal: not a git repository (or any of the parent directories): .git
");
    insta::assert_snapshot!(stdout, @"");

    Ok(())
}

#[test]
fn test_wrap_exit_code() -> anyhow::Result<()> {
    let git = make_git()?;

    git.run_with_options(
        &["branchless", "wrap", "check-ref-format", ".."],
        &GitRunOptions {
            expected_exit_code: 1,
            ..Default::default()
        },
    )?;

    Ok(())
}
