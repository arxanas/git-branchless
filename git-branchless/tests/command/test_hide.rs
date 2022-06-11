use lib::testing::{make_git, GitRunOptions};

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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | o 62fc20d create test1.txt
        |
        @ fe65c1f create test2.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["hide", &test1_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        Hid commit: 62fc20d create test1.txt
        To unhide this 1 commit, run: git undo
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, stderr) = git.run_with_options(
            &["hide", "abc123"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @"Commit not found: abc123
");
    }

    Ok(())
}

#[test]
fn test_hide_already_hidden_commit() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;

    git.run(&["hide", &test1_oid.to_string()])?;
    {
        let (stdout, _stderr) = git.run(&["hide", &test1_oid.to_string()])?;
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
    git.run(&["hide", "HEAD"])?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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

    git.run(&["hide", &test1_oid.to_string()])?;
    git.run(&["hide", &test3_oid.to_string()])?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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

    git.run(&["hide", &test3_oid.to_string()])?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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

    let (stdout, _stderr) = git.run(&["hide", "test", "test^"])?;
    insta::assert_snapshot!(stdout, @r###"
    Hid commit: 62fc20d create test1.txt
    Hid commit: 96d1c37 create test2.txt
    Abandoned 1 branch: test
    To unhide these 2 commits, run: git undo
    "###);

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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

    let (stdout, _stderr) = git.run(&["hide", "--delete-branches", "test", "test^"])?;
    insta::assert_snapshot!(stdout, @r###"
    Hid commit: 62fc20d create test1.txt
    Hid commit: 96d1c37 create test2.txt
    branchless: processing 1 update: branch test
    Deleted 1 branch: test
    To unhide these 2 commits and restore 1 branch, run: git undo
    "###);

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        "###);
    }

    git.run_with_options(
        &["undo"],
        &GitRunOptions {
            input: Some("y".to_string()),
            ..Default::default()
        },
    )?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        |\
        | o 62fc20d (test-def) create test1.txt
        |
        o fe65c1f (test-abc) create test2.txt
        "###);
    }

    let (stdout, _stderr) = git.run(&[
        "hide",
        "--delete-branches",
        &test1_oid.to_string(),
        &test2_oid.to_string(),
    ])?;
    insta::assert_snapshot!(stdout, @r###"
    Hid commit: 62fc20d create test1.txt
    Hid commit: fe65c1f create test2.txt
    branchless: processing 2 updates: branch test-abc, branch test-def
    Deleted 2 branches: test-abc, test-def
    To unhide these 2 commits and restore 2 branches, run: git undo
    "###);

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
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
        let (stdout, _stderr) = git.run(&["unhide", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        Unhid commit: 96d1c37 create test2.txt
        (It was not hidden, so this operation had no effect.)
        To hide this 1 commit, run: git undo
        "###);
    }

    git.run(&["hide", &test2_oid.to_string()])?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        |
        o 62fc20d create test1.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["unhide", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        Unhid commit: 96d1c37 create test2.txt
        To hide this 1 commit, run: git undo
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["hide", "-r", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        Hid commit: 96d1c37 create test2.txt
        Hid commit: 70deb1e create test3.txt
        To unhide these 2 commits, run: git undo
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        |
        o 62fc20d create test1.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["unhide", "-r", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        Unhid commit: 96d1c37 create test2.txt
        Unhid commit: 70deb1e create test3.txt
        To hide these 2 commits, run: git undo
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
