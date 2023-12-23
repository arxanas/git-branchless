use lib::testing::{
    make_git, make_git_with_remote_repo, GitInitOptions, GitRunOptions, GitWrapperWithRemoteRepo,
};

#[test]
fn test_hide_commit() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.run(&["checkout", "master"])?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | o 62fc20d create test1.txt
        |
        @ fe65c1f create test2.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("hide", &[&test1_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        Hid commit: 62fc20d create test1.txt
        To unhide this 1 commit, run: git undo
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ fe65c1f create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_hide_bad_commit() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    {
        let (stdout, stderr) = git.branchless_with_options(
            "hide",
            &["abc123"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"Evaluation error for expression 'abc123': no commit, branch, or reference with the name 'abc123' could be found
");
        insta::assert_snapshot!(stdout, @"");
    }

    Ok(())
}

#[test]
fn test_hide_already_hidden_commit() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;

    git.branchless("hide", &[&test1_oid.to_string()])?;
    {
        let (stdout, _stderr) = git.branchless("hide", &[&test1_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        Hid commit: 62fc20d create test1.txt
        (It was already hidden, so this operation had no effect.)
        To unhide this 1 commit, run: git undo
        "###);
    }

    Ok(())
}

#[test]
fn test_hide_current_commit() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test", 1)?;
    git.branchless("hide", &["HEAD"])?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        % 3df4b93 (manually hidden) create test.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_hidden_commit_with_head_as_child() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.run(&["checkout", &test2_oid.to_string()])?;

    git.branchless("hide", &[&test1_oid.to_string()])?;
    git.branchless("hide", &[&test3_oid.to_string()])?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        x 62fc20d (manually hidden) create test1.txt
        |
        @ 96d1c37 create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_hide_master_commit_with_hidden_children() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.detach_head()?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;

    git.branchless("hide", &[&test3_oid.to_string()])?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 20230db (> master) create test5.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_branches_always_visible() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["branch", "test"])?;
    git.run(&["checkout", "master"])?;

    let (stdout, _stderr) = git.branchless("hide", &["--no-delete-branches", "test", "test^"])?;
    insta::assert_snapshot!(stdout, @r###"
    Hid commit: 62fc20d create test1.txt
    Hid commit: 96d1c37 create test2.txt
    Abandoned 1 branch: test
    To unhide these 2 commits, run: git undo
    "###);

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        |
        x 62fc20d (manually hidden) create test1.txt
        |
        x 96d1c37 (manually hidden) (test) create test2.txt
        "###);
    }

    git.run(&["branch", "-D", "test"])?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @"@ f777ecc (> master) create initial.txt
");
    }

    Ok(())
}

#[test]
fn test_hide_delete_branches() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["branch", "test"])?;
    git.run(&["checkout", "master"])?;

    let (stdout, _stderr) = git.branchless("hide", &["test", "test^"])?;
    insta::assert_snapshot!(stdout, @r###"
    Hid commit: 62fc20d create test1.txt
    Hid commit: 96d1c37 create test2.txt
    branchless: processing 1 update: branch test
    Deleted 1 branch: test
    To unhide these 2 commits and restore 1 branch, run: git undo
    "###);

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        "###);
    }

    git.branchless_with_options(
        "undo",
        &[],
        &GitRunOptions {
            input: Some("y".to_string()),
            ..Default::default()
        },
    )?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 (test) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_hide_delete_multiple_branches() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.run(&["checkout", "master"])?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["checkout", "master"])?;
    // These branches are created "out of order" to confirm the sorting in the output/snapshot.
    git.run(&["branch", "test-def", &test1_oid.to_string()])?;
    git.run(&["branch", "test-abc", &test2_oid.to_string()])?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        |\
        | o 62fc20d (test-def) create test1.txt
        |
        o fe65c1f (test-abc) create test2.txt
        "###);
    }

    let (stdout, _stderr) =
        git.branchless("hide", &[&test1_oid.to_string(), &test2_oid.to_string()])?;
    insta::assert_snapshot!(stdout, @r###"
    Hid commit: 62fc20d create test1.txt
    Hid commit: fe65c1f create test2.txt
    branchless: processing 2 updates: branch test-abc, branch test-def
    Deleted 2 branches: test-abc, test-def
    To unhide these 2 commits and restore 2 branches, run: git undo
    "###);

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_hide_delete_checked_out_branch() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["checkout", "-b", "test"])?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    let (stdout, _stderr) = git.branchless("hide", &["test"])?;
    insta::assert_snapshot!(stdout, @r###"
    Hid commit: 96d1c37 create test2.txt
    branchless: processing 1 update: branch test
    Deleted 1 branch: test
    To unhide this 1 commit and restore 1 branch, run: git undo
    "###);

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        % 96d1c37 (manually hidden) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_unhide() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["checkout", "master"])?;

    {
        let (stdout, _stderr) = git.branchless("unhide", &[&test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        Unhid commit: 96d1c37 create test2.txt
        (It was not hidden, so this operation had no effect.)
        To hide this 1 commit, run: git undo
        "###);
    }

    git.branchless("hide", &[&test2_oid.to_string()])?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        |
        o 62fc20d create test1.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("unhide", &[&test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        Unhid commit: 96d1c37 create test2.txt
        To hide this 1 commit, run: git undo
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_hide_recursive() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.run(&["checkout", "master"])?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        o 70deb1e create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("hide", &["-r", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        Hid commit: 96d1c37 create test2.txt
        Hid commit: 70deb1e create test3.txt
        To unhide these 2 commits, run: git undo
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        |
        o 62fc20d create test1.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("unhide", &["-r", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        Unhid commit: 96d1c37 create test2.txt
        Unhid commit: 70deb1e create test3.txt
        To hide these 2 commits, run: git undo
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        o 70deb1e create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_smartlog_active_non_head_main_branch_commit() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;

    let test1_oid = {
        original_repo.init_repo()?;
        let test1_oid = original_repo.commit_file("test1", 1)?;
        original_repo.commit_file("test2", 2)?;
        original_repo.commit_file("test3", 3)?;

        original_repo.clone_repo_into(&cloned_repo, &[])?;

        test1_oid
    };

    {
        cloned_repo.init_repo_with_options(&GitInitOptions {
            make_initial_commit: false,
            ..Default::default()
        })?;
        // Ensure that the `test1` commit isn't visible just because it's been
        // un-hidden. It's a public commit, so it should be hidden if possible.
        cloned_repo.branchless("unhide", &[&test1_oid.to_string()])?;

        let stdout = cloned_repo.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 70deb1e (> master) create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_smartlog_show_hidden_commits() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["commit", "--amend", "-m", "amended test2"])?;
    let test2_oid_amended = git.get_repo()?.get_head_info()?.oid.unwrap();
    git.branchless("hide", &["HEAD"])?;
    git.run(&["checkout", "HEAD^"])?;

    {
        let (stdout, stderr) = git.branchless(
            "smartlog",
            &[
                "--hidden",
                &format!("{test1_oid} + {test2_oid} + {test2_oid_amended}"),
            ],
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d (master) create test1.txt
        |\
        | x cb8137a (manually hidden) amended test2
        |
        x 96d1c37 (rewritten as cb8137ad) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_smartlog_show_only_branches() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test3", 3)?;
    git.detach_head()?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test5", 5)?;
    git.detach_head()?;
    let test6_oid = git.commit_file("test6", 6)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test7", 7)?;
    git.detach_head()?;
    git.commit_file("test8", 8)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test9", 9)?;

    git.run(&["branch", "branch-2", &test2_oid.to_string()])?;
    git.run(&["branch", "branch-4", &test4_oid.to_string()])?;
    git.branchless("hide", &["--no-delete-branches", &test4_oid.to_string()])?;
    git.branchless("hide", &[&test6_oid.to_string()])?;

    // confirm our baseline:
    // branch, hidden branch and non-branch head are visible; hidden non-branch head is not
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d create test1.txt
        |\
        | o 96d1c37 (branch-2) create test2.txt
        |
        O 4838e49 create test3.txt
        |\
        : x a248207 (manually hidden) (branch-4) create test4.txt
        :
        O 8577a96 create test7.txt
        |\
        | o e8b6a38 create test8.txt
        |
        @ 1b854ed (> master) create test9.txt
        "###);
    }

    // just branches (normal and hidden) but no non-branch heads
    {
        let (stdout, _stderr) = git.branchless("smartlog", &["branches()"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d create test1.txt
        |\
        | o 96d1c37 (branch-2) create test2.txt
        |
        O 4838e49 create test3.txt
        |\
        : x a248207 (manually hidden) (branch-4) create test4.txt
        :
        @ 1b854ed (> master) create test9.txt
        "###);
    }

    Ok(())
}
