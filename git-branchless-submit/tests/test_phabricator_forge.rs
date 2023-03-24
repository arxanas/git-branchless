use std::collections::HashMap;

use git_branchless_testing::{make_git, GitRunOptions};

#[test]
fn test_submit_phabricator_strategy_working_copy() -> eyre::Result<()> {
    let git = make_git()?;
    let env: HashMap<_, _> = git
        .get_base_env(0)
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
        .collect();

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = git.branchless_with_options(
            "submit",
            &["--create", "--forge", "phabricator"],
            &GitRunOptions {
                env,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Using command execution strategy: working-copy
        Attempting rebase in-memory...
        [1/2] Committed as: 55af3db create test1.txt
        [2/2] Committed as: ccb7fd5 create test2.txt
        branchless: processing 2 rewritten commits
        branchless: This operation abandoned 1 commit!
        branchless: Consider running one of the following:
        branchless:   - git restack: re-apply the abandoned commits/branches
        branchless:     (this is most likely what you want to do)
        branchless:   - git smartlog: assess the situation
        branchless:   - git hide [<commit>...]: hide the commits from the smartlog
        branchless:   - git undo: undo the operation
        hint: disable this hint by running: git config --global branchless.hint.restackWarnAbandoned false
        In-memory rebase succeeded.
        [mock-arc] Setting dependencies for Id("0002") to []
        [mock-arc] Setting dependencies for Id("0003") to []
        Created 2 branches: D0002, D0003
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | o 55af3db (D0002) D0002 create test1.txt
        | |
        | o ccb7fd5 (D0003) D0003 create test2.txt
        |
        x 62fc20d (rewritten as 55af3dba) create test1.txt
        |
        @ 1ea08db D0003 create test2.txt
        hint: there is 1 abandoned commit in your commit graph
        hint: to fix this, run: git restack
        hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
        "###);
    }

    Ok(())
}

#[test]
fn test_submit_phabricator_strategy_worktree() -> eyre::Result<()> {
    let git = make_git()?;
    let env: HashMap<_, _> = git
        .get_base_env(0)
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
        .collect();

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = git.branchless_with_options(
            "submit",
            &[
                "--create",
                "--forge",
                "phabricator",
                "--strategy",
                "worktree",
            ],
            &GitRunOptions {
                env,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Using command execution strategy: worktree
        Attempting rebase in-memory...
        [1/2] Committed as: 55af3db create test1.txt
        [2/2] Committed as: ccb7fd5 create test2.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout ccb7fd5d90c1888bea906a41c197e9215d6b9bb3
        In-memory rebase succeeded.
        [mock-arc] Setting dependencies for Id("0002") to []
        [mock-arc] Setting dependencies for Id("0003") to []
        Created 2 branches: D0002, D0003
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 55af3db (D0002) D0002 create test1.txt
        |
        @ ccb7fd5 (D0003) D0003 create test2.txt
        "###);
    }

    Ok(())
}
