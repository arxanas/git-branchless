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

    if !git.supports_reference_transactions()? || git.produces_auto_merge_refs()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = git.branchless("bug-report", &[])?;
        let stdout = redact_timestamp(stdout);

        // Exclude the platform-specific information for this test.
        let stdout = match stdout.split_once("#### Hooks") {
            Some((_, stdout)) => stdout,
            None => &stdout,
        };
        let stdout = stdout.trim();

        insta::assert_snapshot!(stdout, @r###"
        <details>
        <summary>Show 7 hooks</summary>

        ##### Hook `post-applypatch`

        ```
        #!/bin/sh
        ## START BRANCHLESS CONFIG

        git branchless hook post-applypatch "$@"

        ## END BRANCHLESS CONFIG
        ```
        ##### Hook `post-checkout`

        ```
        #!/bin/sh
        ## START BRANCHLESS CONFIG

        git branchless hook post-checkout "$@"

        ## END BRANCHLESS CONFIG
        ```
        ##### Hook `post-commit`

        ```
        #!/bin/sh
        ## START BRANCHLESS CONFIG

        git branchless hook post-commit "$@"

        ## END BRANCHLESS CONFIG
        ```
        ##### Hook `post-merge`

        ```
        #!/bin/sh
        ## START BRANCHLESS CONFIG

        git branchless hook post-merge "$@"

        ## END BRANCHLESS CONFIG
        ```
        ##### Hook `post-rewrite`

        ```
        #!/bin/sh
        ## START BRANCHLESS CONFIG

        git branchless hook post-rewrite "$@"

        ## END BRANCHLESS CONFIG
        ```
        ##### Hook `pre-auto-gc`

        ```
        #!/bin/sh
        ## START BRANCHLESS CONFIG

        git branchless hook pre-auto-gc "$@"

        ## END BRANCHLESS CONFIG
        ```
        ##### Hook `reference-transaction`

        ```
        #!/bin/sh
        ## START BRANCHLESS CONFIG

        # Avoid canceling the reference transaction in the case that `branchless` fails
        # for whatever reason.
        git branchless hook reference-transaction "$@" || (
        echo 'branchless: Failed to process reference transaction!'
        echo 'branchless: Some events (e.g. branch updates) may have been lost.'
        echo 'branchless: This is a bug. Please report it.'
        )

        ## END BRANCHLESS CONFIG
        ```

        </details>

        #### Events


        <details>
        <summary>Show 5 events</summary>

        ##### Event ID: 6, transaction ID: 4 (message: post-commit)

        1. `CommitEvent { timestamp: <redacted for test>, event_tx_id: Id(4), commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f) }`
        ```
        :
        @ 96d1c37 (> master) xxxxxx xxxxxxxxx
        ```
        ##### Event ID: 4, transaction ID: 3 (message: reference-transaction)

        1. `RefUpdateEvent { timestamp: <redacted for test>, event_tx_id: Id(3), ref_name: ReferenceName("HEAD"), old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e, new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f, message: None }`
        1. `RefUpdateEvent { timestamp: <redacted for test>, event_tx_id: Id(3), ref_name: ReferenceName("refs/heads/master"), old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e, new_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f, message: None }`
        ```
        :
        @ 96d1c37 (> master) xxxxxx xxxxxxxxx
        ```
        ##### Event ID: 3, transaction ID: 2 (message: post-commit)

        1. `CommitEvent { timestamp: <redacted for test>, event_tx_id: Id(2), commit_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e) }`
        ```
        :
        @ 96d1c37 (> master) xxxxxx xxxxxxxxx
        ```
        ##### Event ID: 1, transaction ID: 1 (message: reference-transaction)

        1. `RefUpdateEvent { timestamp: <redacted for test>, event_tx_id: Id(1), ref_name: ReferenceName("HEAD"), old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24, new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e, message: None }`
        1. `RefUpdateEvent { timestamp: <redacted for test>, event_tx_id: Id(1), ref_name: ReferenceName("refs/heads/master"), old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24, new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e, message: None }`
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
