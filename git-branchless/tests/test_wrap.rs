use lib::core::effects::Effects;
use lib::core::eventlog::testing::{get_event_replayer_events, redact_event_timestamp};
use lib::core::eventlog::{Event, EventLogDb, EventReplayer};
use lib::core::formatting::Glyphs;
use lib::git::GitVersion;
use lib::testing::{make_git, GitRunOptions};

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

    git.branchless("wrap", &["rebase", "foo"])?;

    let effects = Effects::new_suppress_for_test(Glyphs::text());
    let repo = git.get_repo()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&effects, &repo, &event_log_db)?;
    let events: Vec<Event> = get_event_replayer_events(&event_replayer)
        .iter()
        .map(|event| redact_event_timestamp(event.clone()))
        .collect();

    // Bug fixed in Git v2.35: https://github.com/git/git/commit/4866a64508465938b7661eb31afbde305d83e234
    let git_version = git.get_version()?;
    if git_version >= GitVersion(2, 36, 0) {
        insta::assert_debug_snapshot!(events, @r###"
        [
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    1,
                ),
                ref_name: ReferenceName(
                    "refs/heads/foo",
                ),
                old_oid: 0000000000000000000000000000000000000000,
                new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    2,
                ),
                ref_name: ReferenceName(
                    "HEAD",
                ),
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    3,
                ),
                ref_name: ReferenceName(
                    "HEAD",
                ),
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    3,
                ),
                ref_name: ReferenceName(
                    "refs/heads/foo",
                ),
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                message: None,
            },
            CommitEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    4,
                ),
                commit_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    5,
                ),
                ref_name: ReferenceName(
                    "HEAD",
                ),
                old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    5,
                ),
                ref_name: ReferenceName(
                    "refs/heads/foo",
                ),
                old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
            CommitEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    6,
                ),
                commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    7,
                ),
                ref_name: ReferenceName(
                    "HEAD",
                ),
                old_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    8,
                ),
                ref_name: ReferenceName(
                    "HEAD",
                ),
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    8,
                ),
                ref_name: ReferenceName(
                    "HEAD",
                ),
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    8,
                ),
                ref_name: ReferenceName(
                    "refs/heads/master",
                ),
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
        ]
        "###);
    } else if git_version < GitVersion(2, 35, 0) {
        insta::assert_debug_snapshot!(events, @r###"
        [
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    1,
                ),
                ref_name: ReferenceName(
                    "refs/heads/foo",
                ),
                old_oid: 0000000000000000000000000000000000000000,
                new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    2,
                ),
                ref_name: ReferenceName(
                    "HEAD",
                ),
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    3,
                ),
                ref_name: ReferenceName(
                    "HEAD",
                ),
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    3,
                ),
                ref_name: ReferenceName(
                    "refs/heads/foo",
                ),
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                message: None,
            },
            CommitEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    4,
                ),
                commit_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    5,
                ),
                ref_name: ReferenceName(
                    "HEAD",
                ),
                old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    5,
                ),
                ref_name: ReferenceName(
                    "refs/heads/foo",
                ),
                old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
            CommitEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    6,
                ),
                commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    7,
                ),
                ref_name: ReferenceName(
                    "HEAD",
                ),
                old_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    8,
                ),
                ref_name: ReferenceName(
                    "HEAD",
                ),
                old_oid: 0000000000000000000000000000000000000000,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    8,
                ),
                ref_name: ReferenceName(
                    "HEAD",
                ),
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
            RefUpdateEvent {
                timestamp: 0.0,
                event_tx_id: EventTransactionId(
                    8,
                ),
                ref_name: ReferenceName(
                    "refs/heads/master",
                ),
                old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                message: None,
            },
        ]
        "###);
    }

    Ok(())
}

#[test]
fn test_wrap_explicit_git_executable() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    let (stdout, _stderr) = git.branchless(
        "wrap",
        &[
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
        ],
    )?;
    assert!(stdout.contains("Branchless workflow for Git."));
    Ok(())
}

#[test]
fn test_wrap_without_repo() -> eyre::Result<()> {
    let git = make_git()?;

    let (stdout, stderr) = git.branchless_with_options(
        "wrap",
        &["status"],
        &GitRunOptions {
            expected_exit_code: 128,
            ..Default::default()
        },
    )?;
    println!("{}", &stderr);
    assert!(stderr.contains("fatal: not a git repository"));
    insta::assert_snapshot!(stdout, @"");

    Ok(())
}

#[test]
fn test_wrap_exit_code() -> eyre::Result<()> {
    let git = make_git()?;

    git.branchless_with_options(
        "wrap",
        &["check-ref-format", ".."],
        &GitRunOptions {
            expected_exit_code: 1,
            ..Default::default()
        },
    )?;

    Ok(())
}
