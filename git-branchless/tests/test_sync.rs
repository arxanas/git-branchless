use lib::testing::{
    make_git, make_git_with_remote_repo, remove_nondeterministic_lines, GitInitOptions,
    GitRunOptions, GitWrapperWithRemoteRepo,
};

#[test]
fn test_sync_basic() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
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
        let stdout = git.smartlog()?;
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
        let (stdout, stderr) = git.branchless("sync", &[])?;
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
        let stdout = git.smartlog()?;
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

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.detach_head()?;
    git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;

    {
        let (stdout, stderr) = git.branchless("sync", &[])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @"Not moving up-to-date stack at 70deb1e create test3.txt
");
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
    if !original_repo.supports_reference_transactions()? {
        return Ok(());
    }

    original_repo.init_repo()?;
    original_repo.commit_file("test1", 1)?;
    original_repo.commit_file("test2", 2)?;

    original_repo.clone_repo_into(&cloned_repo, &["--branch", "master"])?;
    cloned_repo.init_repo_with_options(&GitInitOptions {
        make_initial_commit: false,
        ..Default::default()
    })?;
    cloned_repo.detach_head()?;

    original_repo.commit_file("test3", 3)?;
    original_repo.commit_file("test4", 4)?;
    original_repo.commit_file("test5", 5)?;
    cloned_repo.commit_file("test3", 3)?;
    cloned_repo.commit_file("test4", 4)?;
    cloned_repo.commit_file("test6", 6)?;

    {
        let stdout = cloned_repo.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 96d1c37 (master) create test2.txt
        |
        o 70deb1e create test3.txt
        |
        o 355e173 create test4.txt
        |
        @ 6ac5566 create test6.txt
        "###);
    }

    {
        let (stdout, _stderr) = cloned_repo.branchless("sync", &["-p"])?;
        let stdout: String = remove_nondeterministic_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> fetch origin
        Fast-forwarding branch master to f81d55c create test5.txt
        Attempting rebase in-memory...
        [1/1] Committed as: 2831fb5 create test6.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout 2831fb5864ee099dc3e448a38dcb3c8527149510
        In-memory rebase succeeded.
        Synced 6ac5566 create test6.txt
        "###);
    }

    {
        let stdout = cloned_repo.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O f81d55c (master) create test5.txt
        |
        @ 2831fb5 create test6.txt
        "###);
    }

    {
        let (stdout, _stderr) = cloned_repo.branchless("sync", &["-p"])?;
        let stdout: String = remove_nondeterministic_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> fetch origin
        Not updating branch master at f81d55c create test5.txt
        Not moving up-to-date stack at 2831fb5 create test6.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_sync_stack_from_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "-b", "foo"])?;
    git.commit_file("test3", 3)?;
    git.commit_file("test3-1", 3)?;
    git.run(&["checkout", "-b", "bar", "master"])?;
    git.commit_file("test4", 4)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test5", 5)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 96d1c37 create test2.txt
        |\
        | o 70deb1e create test3.txt
        | |
        | o 10523fe (foo) create test3-1.txt
        |\
        | o f57e36f (bar) create test4.txt
        |
        @ d2e18e3 (> master) create test5.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("sync", &["foo"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/2] Committed as: 8e521a1 create test3.txt
        [2/2] Committed as: 11bfd99 create test3-1.txt
        branchless: processing 1 update: branch foo
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout master
        In-memory rebase succeeded.
        Synced 70deb1e create test3.txt
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 96d1c37 create test2.txt
        |\
        | o f57e36f (bar) create test4.txt
        |
        @ d2e18e3 (> master) create test5.txt
        |
        o 8e521a1 create test3.txt
        |
        o 11bfd99 (foo) create test3-1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_sync_divergent_main_branch() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;
    if !original_repo.supports_reference_transactions()? {
        return Ok(());
    }

    original_repo.init_repo()?;
    original_repo.commit_file("test1", 1)?;
    original_repo.commit_file("test2", 2)?;

    original_repo.clone_repo_into(&cloned_repo, &["--branch", "master"])?;
    cloned_repo.init_repo_with_options(&GitInitOptions {
        make_initial_commit: false,
        ..Default::default()
    })?;

    original_repo.commit_file("test3", 3)?;
    original_repo.commit_file("test4", 4)?;
    cloned_repo.commit_file("test5", 5)?;

    {
        let stdout = cloned_repo.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ d2e18e3 (> master) create test5.txt
        "###);
    }

    {
        let (stdout, _stderr) = cloned_repo.branchless("sync", &["-p"])?;
        let stdout = remove_nondeterministic_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> fetch origin
        Syncing branch master
        Attempting rebase in-memory...
        [1/1] Committed as: f81d55c create test5.txt
        branchless: processing 1 update: branch master
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout master
        Your branch is ahead of 'origin/master' by 1 commit.
          (use "git push" to publish your local commits)
        In-memory rebase succeeded.
        Synced d2e18e3 create test5.txt
        "###);
    }

    {
        let stdout = cloned_repo.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ f81d55c (> master) create test5.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_sync_no_delete_main_branch() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;
    if !original_repo.supports_reference_transactions()? {
        return Ok(());
    }

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
    cloned_repo.run(&["reset", "--hard", "HEAD^"])?;

    // Simulate landing the commit upstream with a potentially different commit
    // hash.
    cloned_repo.run(&["cherry-pick", "origin/master"])?;
    cloned_repo.run(&["commit", "--amend", "-m", "updated commit message"])?;

    cloned_repo.run(&["branch", "should-be-deleted"])?;

    {
        let (stdout, stderr) = cloned_repo.branchless("sync", &["-p", "--on-disk"])?;
        let stdout = remove_nondeterministic_lines(stdout);
        let stderr = remove_nondeterministic_lines(stderr);
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 1 update: ref HEAD
        Executing: git branchless hook-skip-upstream-applied-commit 6ffd720862b7ae71cbe30d66ed27ea8579e24b0f
        Executing: git branchless hook-register-extra-post-rewrite-hook
        branchless: processing 1 rewritten commit
        branchless: processing 2 updates: branch master, branch should-be-deleted
        branchless: creating working copy snapshot
        branchless: running command: <git-executable> checkout master
        branchless: processing checkout
        :
        @ 96d1c37 (> master) create test2.txt
        Successfully rebased and updated detached HEAD.
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> fetch origin
        Syncing branch master
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Skipping commit (was already applied upstream): 6ffd720 updated commit message
        Synced 6ffd720 updated commit message
        "###);
    }

    {
        let stdout = cloned_repo.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 96d1c37 (> master) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_sync_merge_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.run(&["checkout", "-b", "foo"])?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "--detach", "master"])?;
    git.commit_file("test3", 3)?;
    git.run(&["merge", "foo"])?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test4", 4)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
        | o 62fc20d create test1.txt
        | |
        | o 96d1c37 (foo) create test2.txt
        | & (merge) 5cedb02 Merge branch 'foo' into HEAD
        |\
        | o 98b9119 create test3.txt
        | |
        | | & (merge) 96d1c37 (foo) create test2.txt
        | |/
        | o 5cedb02 Merge branch 'foo' into HEAD
        |
        @ 8f7aef5 (> master) create test4.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("sync", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        Attempting rebase in-memory...
        Can't rebase merge commit in-memory: 62fc20d create test1.txt
        Can't rebase merge commit in-memory: 98b9119 create test3.txt
        "###);
    }

    Ok(())
}

/// Regression test for https://github.com/arxanas/git-branchless/issues/838
#[test]
fn test_sync_checked_out_main_branch() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;
    if !original_repo.supports_reference_transactions()? {
        return Ok(());
    }

    original_repo.init_repo()?;
    original_repo.commit_file("test1", 1)?;

    original_repo.clone_repo_into(&cloned_repo, &["--branch", "master"])?;
    cloned_repo.init_repo_with_options(&GitInitOptions {
        make_initial_commit: false,
        ..Default::default()
    })?;

    original_repo.commit_file("test2", 2)?;
    {
        let (stdout, _stderr) = cloned_repo.branchless("sync", &["--pull"])?;
        let stdout: String = remove_nondeterministic_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> fetch origin
        Fast-forwarding branch master to 96d1c37 create test2.txt
        branchless: running command: <git-executable> rebase 96d1c37a3d4363611c49f7e52186e189a04c531f
        "###);
    }

    {
        let (stdout, _stderr) = cloned_repo.run(&["status"])?;
        insta::assert_snapshot!(stdout, @r###"
            On branch master
            Your branch is up to date with 'origin/master'.

            nothing to commit, working tree clean
            "###);
    }

    Ok(())
}

#[test]
fn test_sync_checked_out_main_with_dirty_working_copy() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;
    if !original_repo.supports_reference_transactions()? {
        return Ok(());
    }

    original_repo.init_repo()?;
    original_repo.commit_file("test1", 1)?;

    original_repo.clone_repo_into(&cloned_repo, &["--branch", "master"])?;
    cloned_repo.init_repo_with_options(&GitInitOptions {
        make_initial_commit: false,
        ..Default::default()
    })?;

    cloned_repo.write_file_txt("test1", "new contents, do not overwrite\n")?;
    {
        let (stdout, stderr) = cloned_repo.branchless_with_options(
            "sync",
            &["--pull"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        let stdout: String = remove_nondeterministic_lines(stdout);
        insta::assert_snapshot!(stderr, @r###"
        error: cannot rebase: You have unstaged changes.
        error: Please commit or stash them.
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> fetch origin
        Not updating branch master at 62fc20d create test1.txt
        branchless: running command: <git-executable> rebase 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
        "###);
    }

    Ok(())
}
