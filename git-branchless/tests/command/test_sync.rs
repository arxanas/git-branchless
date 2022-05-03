use lib::testing::{make_git, make_git_with_remote_repo, GitInitOptions, GitWrapperWithRemoteRepo};

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
        O f777ecc create initial.txt
        |\
        | o 62fc20d create test1.txt
        | |
        | o 96d1c37 create test2.txt
        |
        O 98b9119 create test3.txt
        |\
        | o 2b633ed create test4.txt
        |
        @ 117e086 (> master) create test5.txt
        "###);
    }

    {
        let (stdout, stderr) = git.run(&["sync"])?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: creating working copy snapshot
        Switched to branch 'master'
        branchless: processing checkout
        branchless: creating working copy snapshot
        Switched to branch 'master'
        branchless: processing checkout
        "###);
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/2] Committed as: 87c7a36 create test1.txt
        [2/2] Committed as: 8ee4f26 create test2.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout master
        In-memory rebase succeeded.
        Attempting rebase in-memory...
        [1/1] Committed as: d7e7e6c create test4.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout master
        In-memory rebase succeeded.
        Synced 62fc20d create test1.txt
        Synced 2b633ed create test4.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 117e086 (> master) create test5.txt
        |\
        | o 87c7a36 create test1.txt
        | |
        | o 8ee4f26 create test2.txt
        |
        o d7e7e6c create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_sync_up_to_date() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.detach_head()?;
    git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;

    {
        let (stdout, stderr) = git.run(&["sync"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @"Not moving up-to-date stack at 70deb1e create test3.txt
");
    }

    {
        let (stdout, stderr) = git.run(&["sync", "-f"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/2] Committed as: 70deb1e create test3.txt
        [2/2] Committed as: 355e173 create test4.txt
        branchless: processing 2 rewritten commits
        In-memory rebase succeeded.
        Synced 70deb1e create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_sync_pull() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;

    original_repo.init_repo()?;
    original_repo.commit_file("test1", 1)?;
    original_repo.commit_file("test2", 2)?;
    original_repo.run(&["checkout", "-b", "foo"])?;
    original_repo.commit_file("test3", 3)?;

    original_repo.clone_repo_into(&cloned_repo, &["--branch", "master"])?;
    cloned_repo.init_repo_with_options(&GitInitOptions {
        make_initial_commit: false,
        ..Default::default()
    })?;
    cloned_repo.run(&["checkout", "foo"])?;

    original_repo.commit_file("test4", 4)?;
    original_repo.run(&["checkout", "master"])?;
    original_repo.commit_file("test5", 5)?;

    {
        let (stdout, _stderr) = cloned_repo.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 96d1c37 (master, remote origin/master) create test2.txt
        |
        @ 70deb1e (> foo) create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = cloned_repo.run(&["sync", "-p"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> fetch --all
        Fetching origin
        Attempting rebase in-memory...
        [1/1] Committed as: 8e521a1 create test3.txt
        branchless: processing 1 update: branch foo
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout foo
        Your branch and 'origin/foo' have diverged,
        and have 2 and 2 different commits each, respectively.
          (use "git pull" to merge the remote branch into yours)
        In-memory rebase succeeded.
        Synced 70deb1e create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = cloned_repo.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 96d1c37 (master) create test2.txt
        |
        O d2e18e3 (remote origin/master) create test5.txt
        |
        @ 8e521a1 (> foo) create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_sync_specific_commit() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "-b", "foo"])?;
    git.commit_file("test3", 3)?;
    git.run(&["checkout", "-b", "bar", "master"])?;
    git.commit_file("test4", 4)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test5", 5)?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 96d1c37 create test2.txt
        |\
        | o 70deb1e (foo) create test3.txt
        |\
        | o f57e36f (bar) create test4.txt
        |
        @ d2e18e3 (> master) create test5.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["sync", "foo"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 8e521a1 create test3.txt
        branchless: processing 1 update: branch foo
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout master
        In-memory rebase succeeded.
        Synced 70deb1e create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 96d1c37 create test2.txt
        |\
        | o f57e36f (bar) create test4.txt
        |
        @ d2e18e3 (> master) create test5.txt
        |
        o 8e521a1 (foo) create test3.txt
        "###);
    }

    Ok(())
}
