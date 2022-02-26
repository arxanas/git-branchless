use branchless::testing::make_git;

#[test]
fn test_sync_basic() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    git.run(&["checkout", "master"])?;
    git.commit_file("test3", 3)?;

    git.detach_head()?;
    git.commit_file("test4", 4)?;

    git.run(&["checkout", "master"])?;
    git.commit_file("test5", 5)?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 create initial.txt
        |\
        | o 62fc20d2 create test1.txt
        | |
        | o 96d1c37a create test2.txt
        |
        O 98b9119d create test3.txt
        |\
        | o 2b633ed7 create test4.txt
        |
        @ 117e0866 (master) create test5.txt
        "###);
    }

    {
        let (stdout, stderr) = git.run(&["sync"])?;
        insta::assert_snapshot!(stderr, @r###"
        Switched to branch 'master'
        branchless: processing checkout
        Switched to branch 'master'
        branchless: processing checkout
        "###);
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/2] Committed as: 87c7a36c create test1.txt
        [2/2] Committed as: 8ee4f266 create test2.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout master
        :
        O 98b9119d create test3.txt
        |\
        | o 2b633ed7 create test4.txt
        |
        @ 117e0866 (master) create test5.txt
        |
        o 87c7a36c create test1.txt
        |
        o 8ee4f266 create test2.txt
        In-memory rebase succeeded.
        Attempting rebase in-memory...
        [1/1] Committed as: d7e7e6c3 create test4.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout master
        :
        @ 117e0866 (master) create test5.txt
        |\
        | o 87c7a36c create test1.txt
        | |
        | o 8ee4f266 create test2.txt
        |
        o d7e7e6c3 create test4.txt
        In-memory rebase succeeded.
        Synced 62fc20d2 create test1.txt
        Synced 2b633ed7 create test4.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 117e0866 (master) create test5.txt
        |\
        | o 87c7a36c create test1.txt
        | |
        | o 8ee4f266 create test2.txt
        |
        o d7e7e6c3 create test4.txt
        "###);
    }

    Ok(())
}
