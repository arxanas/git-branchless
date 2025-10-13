use std::collections::HashMap;

use lib::testing::{make_git, GitRunOptions, GitWrapper};

fn mock_env(git: &GitWrapper) -> HashMap<String, String> {
    git.get_base_env(0)
        .into_iter()
        .map(|(k, v)| {
            (
                k.to_str().unwrap().to_string(),
                v.to_str().unwrap().to_string(),
            )
        })
        .chain([(
            git_branchless_submit::phabricator::SHOULD_MOCK_ENV_KEY.to_string(),
            "1".to_string(),
        )])
        .collect()
}

#[test]
fn test_submit_phabricator_strategy_working_copy() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    git.write_file_txt("test1", "uncommitted changes\n")?;
    {
        let (stdout, stderr) = git.branchless_with_options(
            "submit",
            &["--create", "--forge", "phabricator"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        This operation would modify the working copy, but you have uncommitted changes
        in your working copy which might be overwritten as a result.
        Commit your changes and then try again.
        "###);
    }
    git.run(&["checkout", "--", "."])?;

    {
        let (stdout, _stderr) = git.branchless_with_options(
            "submit",
            &["--create", "--forge", "phabricator"],
            &GitRunOptions {
                env: mock_env(&git),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Using command execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Attempting rebase in-memory...
        [1/2] Committed as: 55af3db create test1.txt
        [2/2] Committed as: ccb7fd5 create test2.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout ccb7fd5d90c1888bea906a41c197e9215d6b9bb3 --
        In-memory rebase succeeded.
        Setting D0002 as stack root (no dependencies)
        Stacking D0003 on top of D0002
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Using command execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Submitted 2 commits: D0002, D0003
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 55af3db D0002 create test1.txt
        |
        @ ccb7fd5 D0003 create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_submit_phabricator_strategy_worktree() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 96d1c37 create test2.txt
        "###);
    }

    {
        let (stdout, stderr) = git.branchless_with_options(
            "submit",
            &[
                "--create",
                "--forge",
                "phabricator",
                "--strategy",
                "worktree",
            ],
            &GitRunOptions {
                env: mock_env(&git),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: creating working copy snapshot
        Previous HEAD position was 96d1c37 create test2.txt
        branchless: processing 1 update: ref HEAD
        HEAD is now at ccb7fd5 create test2.txt
        branchless: processing checkout
        "###);
        insta::assert_snapshot!(stdout, @r###"
        Using command execution strategy: worktree
        Attempting rebase in-memory...
        [1/2] Committed as: 55af3db create test1.txt
        [2/2] Committed as: ccb7fd5 create test2.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout ccb7fd5d90c1888bea906a41c197e9215d6b9bb3 --
        In-memory rebase succeeded.
        Setting D0002 as stack root (no dependencies)
        Stacking D0003 on top of D0002
        Using command execution strategy: worktree
        Submitted 2 commits: D0002, D0003
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 55af3db D0002 create test1.txt
        |
        @ ccb7fd5 D0003 create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_submit_phabricator_update() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = git.branchless_with_options(
            "submit",
            &["--create", "--forge", "phabricator"],
            &GitRunOptions {
                env: mock_env(&git),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Using command execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Attempting rebase in-memory...
        [1/2] Committed as: 55af3db create test1.txt
        [2/2] Committed as: ccb7fd5 create test2.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout ccb7fd5d90c1888bea906a41c197e9215d6b9bb3 --
        In-memory rebase succeeded.
        Setting D0002 as stack root (no dependencies)
        Stacking D0003 on top of D0002
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Using command execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Submitted 2 commits: D0002, D0003
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless_with_options(
            "submit",
            &["--forge", "phabricator"],
            &GitRunOptions {
                env: mock_env(&git),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Using command execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Setting D0002 as stack root (no dependencies)
        Stacking D0003 on top of D0002
        "###);
    }

    Ok(())
}
