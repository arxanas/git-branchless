use branchless::testing::{with_git, GitRunOptions};

#[test]
fn test_hide_commit() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.run(&["checkout", "master"])?;
        git.detach_head()?;
        git.commit_file("test2", 2)?;

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            O f777ecc9 (master) create initial.txt
            |\
            | o 62fc20d2 create test1.txt
            |
            @ fe65c1fe create test2.txt
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["hide", &test1_oid.to_string()])?;
            insta::assert_snapshot!(stdout, @r###"
            Hid commit: 62fc20d2 create test1.txt
            To unhide this commit, run: git unhide 62fc20d2
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            O f777ecc9 (master) create initial.txt
            |
            @ fe65c1fe create test2.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_hide_bad_commit() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;

        {
            let (stdout, _stderr) = git.run_with_options(
                &["hide", "abc123"],
                &GitRunOptions {
                    expected_exit_code: 1,
                    ..Default::default()
                },
            )?;
            insta::assert_snapshot!(stdout, @"Commit not found: abc123");
        }

        Ok(())
    })
}

#[test]
fn test_hide_already_hidden_commit() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;

        git.run(&["hide", &test1_oid.to_string()])?;
        {
            let (stdout, _stderr) = git.run(&["hide", &test1_oid.to_string()])?;
            insta::assert_snapshot!(stdout, @r###"
            Hid commit: 62fc20d2 create test1.txt
            (It was already hidden, so this operation had no effect.)
            To unhide this commit, run: git unhide 62fc20d2
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_hide_current_commit() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.detach_head()?;
        git.commit_file("test", 1)?;
        git.run(&["hide", "HEAD"])?;

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            O f777ecc9 (master) create initial.txt
            |
            % 3df4b935 create test.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_hidden_commit_with_head_as_child() -> anyhow::Result<()> {
    with_git(|git| {
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
            O f777ecc9 (master) create initial.txt
            |
            x 62fc20d2 create test1.txt
            |
            @ 96d1c37a create test2.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_hide_master_commit_with_hidden_children() -> anyhow::Result<()> {
    with_git(|git| {
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
            @ 20230db7 (master) create test5.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_branches_always_visible() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.detach_head()?;
        git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        git.run(&["branch", "test"])?;
        git.run(&["checkout", "master"])?;

        git.run(&["hide", "test", "test^"])?;
        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            @ f777ecc9 (master) create initial.txt
            |
            x 62fc20d2 create test1.txt
            |
            x 96d1c37a (test) create test2.txt
            "###);
        }

        git.run(&["branch", "-D", "test"])?;
        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @"@ f777ecc9 (master) create initial.txt
");
        }

        Ok(())
    })
}

#[test]
fn test_unhide() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.detach_head()?;
        git.commit_file("test1", 1)?;
        let test2_oid = git.commit_file("test2", 2)?;
        git.run(&["checkout", "master"])?;

        {
            let (stdout, _stderr) = git.run(&["unhide", &test2_oid.to_string()])?;
            insta::assert_snapshot!(stdout, @r###"
            Unhid commit: 96d1c37a create test2.txt
            (It was not hidden, so this operation had no effect.)
            To hide this commit, run: git hide 96d1c37a
            "###);
        }

        git.run(&["hide", &test2_oid.to_string()])?;
        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            @ f777ecc9 (master) create initial.txt
            |
            o 62fc20d2 create test1.txt
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["unhide", &test2_oid.to_string()])?;
            insta::assert_snapshot!(stdout, @r###"
            Unhid commit: 96d1c37a create test2.txt
            To hide this commit, run: git hide 96d1c37a
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            @ f777ecc9 (master) create initial.txt
            |
            o 62fc20d2 create test1.txt
            |
            o 96d1c37a create test2.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_hide_recursive() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.detach_head()?;
        git.commit_file("test1", 1)?;
        let test2_oid = git.commit_file("test2", 2)?;
        git.commit_file("test3", 3)?;
        git.run(&["checkout", "master"])?;

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            @ f777ecc9 (master) create initial.txt
            |
            o 62fc20d2 create test1.txt
            |
            o 96d1c37a create test2.txt
            |
            o 70deb1e2 create test3.txt
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["hide", "-r", &test2_oid.to_string()])?;
            insta::assert_snapshot!(stdout, @r###"
            Hid commit: 96d1c37a create test2.txt
            To unhide this commit, run: git unhide 96d1c37a
            Hid commit: 70deb1e2 create test3.txt
            To unhide this commit, run: git unhide 70deb1e2
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            @ f777ecc9 (master) create initial.txt
            |
            o 62fc20d2 create test1.txt
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["unhide", "-r", &test2_oid.to_string()])?;
            insta::assert_snapshot!(stdout, @r###"
            Unhid commit: 96d1c37a create test2.txt
            To hide this commit, run: git hide 96d1c37a
            Unhid commit: 70deb1e2 create test3.txt
            To hide this commit, run: git hide 70deb1e2
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            @ f777ecc9 (master) create initial.txt
            |
            o 62fc20d2 create test1.txt
            |
            o 96d1c37a create test2.txt
            |
            o 70deb1e2 create test3.txt
            "###);
        }

        Ok(())
    })
}
