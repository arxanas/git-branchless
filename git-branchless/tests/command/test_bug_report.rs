use lazy_static::lazy_static;
use lib::testing::make_git;
use regex::Regex;

lazy_static! {
    static ref TIMESTAMP_RE: Regex = Regex::new("timestamp: ([0-9.]+)").unwrap();
}

fn redact_timestamp(str: String) -> String {
    TIMESTAMP_RE
        .replace_all(&str, "timestamp: <redacted for test>")
        .to_string()
}

#[test]
fn test_bug_report() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = git.run(&["branchless", "bug-report"])?;
        let stdout = redact_timestamp(stdout);

        // Exclude the platform-specific information for this test.
        let stdout = match stdout.split_once("#### Events") {
            Some((_, stdout)) => stdout,
            None => &stdout,
        };
        let stdout = stdout.trim();

        insta::assert_snapshot!(stdout, @r###"
        <details>
        <summary>Show 5 events</summary>

        ##### Event ID: 6, transaction ID: 4

        1. `CommitEvent { timestamp: <redacted for test>, event_tx_id: EventTransactionId(4), commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f) }`
        ```
        :
        @ 96d1c37 (> master) xxxxxx xxxxxxxxx
        ```
        ##### Event ID: 4, transaction ID: 3

        1. `RefUpdateEvent { timestamp: <redacted for test>, event_tx_id: EventTransactionId(3), ref_name: "HEAD", old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e, new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f, message: None }`
        1. `RefUpdateEvent { timestamp: <redacted for test>, event_tx_id: EventTransactionId(3), ref_name: "refs/heads/master", old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e, new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f, message: None }`
        ```
        :
        @ 96d1c37 (> master) xxxxxx xxxxxxxxx
        ```
        ##### Event ID: 3, transaction ID: 2

        1. `CommitEvent { timestamp: <redacted for test>, event_tx_id: EventTransactionId(2), commit_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e) }`
        ```
        :
        @ 96d1c37 (> master) xxxxxx xxxxxxxxx
        ```
        ##### Event ID: 1, transaction ID: 1

        1. `RefUpdateEvent { timestamp: <redacted for test>, event_tx_id: EventTransactionId(1), ref_name: "HEAD", old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24, new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e, message: None }`
        1. `RefUpdateEvent { timestamp: <redacted for test>, event_tx_id: EventTransactionId(1), ref_name: "refs/heads/master", old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24, new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e, message: None }`
        ```
        :
        @ 96d1c37 (> master) xxxxxx xxxxxxxxx
        ```
        There are no previous available events.
        ```
        :
        @ 96d1c37 (> master) xxxxxx xxxxxxxxx
        ```

        </details>
        "###);
    }

    Ok(())
}
