use git_branchless_testing::{make_git, GitInitOptions};
use itertools::Itertools;
use lib::core::eventlog::testing::redact_event_timestamp;
use lib::core::eventlog::EventLogDb;
use lib::git::GitVersion;

#[test]
fn test_gc() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD^"])?;

    {
        let repo = git.get_repo()?;
        assert!(repo.revparse_single_commit("62fc20d2").is_ok());
    }

    git.run(&["gc", "--prune=now"])?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        "###);
    }

    git.branchless("hide", &["62fc20d2"])?;
    {
        let (stdout, _stderr) = git.branchless("gc", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: collecting garbage
        branchless: 1 dangling reference deleted
        "###);
    }

    git.run(&["gc", "--prune=now"])?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @"@ f777ecc (master) create initial.txt
");
    }

    {
        let repo = git.get_repo()?;
        assert!(repo.revparse_single_commit("62fc20d2")?.is_none())
    }

    Ok(())
}

#[test]
fn test_gc_reference_transaction() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    let git_version = git.get_version()?;
    if git_version >= GitVersion(2, 35, 0) {
        // Change in reference-transaction behavior causes this test to fail.
        return Ok(());
    }

    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.branchless("hide", &["HEAD"])?;
    git.run(&["checkout", "HEAD^"])?;

    {
        let (stdout, _stderr) = git.branchless("gc", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: collecting garbage
        branchless: 1 dangling reference deleted
        "###);
    }

    git.run(&["gc", "--prune=now"])?;

    let conn = git.get_repo()?.get_db_conn()?;
    let event_log = EventLogDb::new(&conn)?;
    let events = event_log
        .get_events()?
        .into_iter()
        .map(redact_event_timestamp)
        .collect_vec();
    insta::assert_debug_snapshot!(events, @r###"
    [
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                1,
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
                1,
            ),
            ref_name: ReferenceName(
                "refs/heads/master",
            ),
            old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            message: None,
        },
        CommitEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                2,
            ),
            commit_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                3,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: 0000000000000000000000000000000000000000,
            new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            message: None,
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                4,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            message: None,
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
        CommitEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                6,
            ),
            commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
        },
        ObsoleteEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                7,
            ),
            commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
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
            new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            message: None,
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                9,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
            new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            message: None,
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                10,
            ),
            ref_name: ReferenceName(
                "refs/heads/master",
            ),
            old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            message: None,
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                11,
            ),
            ref_name: ReferenceName(
                "refs/heads/master",
            ),
            old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            message: None,
        },
    ]
    "###);

    Ok(())
}

#[test]
fn test_gc_no_init() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo_with_options(&GitInitOptions {
        run_branchless_init: false,
        ..Default::default()
    })?;

    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 62fc20d create test1.txt
        "###);
    }

    git.run(&["checkout", "HEAD~"])?;
    {
        let (stdout, stderr) = git.run(&["gc", "--prune=now"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @"");
    }

    git.run(&["checkout", &test1_oid.to_string()])?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 62fc20d create test1.txt
        "###);
    }

    git.run(&["checkout", "HEAD~"])?;
    git.branchless("gc", &[])?;
    {
        let (stdout, stderr) = git.run(&["gc", "--prune=now"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @"");
    }

    git.run(&["checkout", &test1_oid.to_string()])?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 62fc20d create test1.txt
        "###);
    }

    Ok(())
}
