use branchless::{
    testing::{with_git, Git},
    util::GitVersion,
};

/// Git v2.24 produces this message on `git move` tests:
///
/// ```text
/// BUG: builtin/rebase.c:1172: Unhandled rebase type 1
/// ```
///
/// I don't know why. `git rebase` in v2.24 supports the `--preserve-merges`
/// option, so we should be able to generate our rebase plans.
fn has_git_v2_24_bug(git: &Git) -> anyhow::Result<bool> {
    let GitVersion(major, minor, _patch) = git.get_version()?;
    Ok((major, minor) <= (2, 24))
}

#[test]
fn test_move_stick_on_disk() -> anyhow::Result<()> {
    with_git(|git| {
        if has_git_v2_24_bug(&git)? {
            return Ok(());
        }

        git.init_repo()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;

        git.detach_head()?;
        let test3_oid = git.commit_file("test3", 3)?;
        git.commit_file("test4", 4)?;

        git.run(&[
            "move",
            "--on-disk",
            "-s",
            &test3_oid.to_string(),
            "-d",
            &test1_oid.to_string(),
        ])?;
        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d2 create test1.txt
            |\
            | o cade1d30 create test3.txt
            | |
            | @ 5bb72580 create test4.txt
            |
            O 96d1c37a (master) create test2.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_move_stick_in_memory() -> anyhow::Result<()> {
    with_git(|git| {
        if has_git_v2_24_bug(&git)? {
            return Ok(());
        }

        git.init_repo()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;

        git.detach_head()?;
        let test3_oid = git.commit_file("test3", 3)?;
        git.commit_file("test4", 4)?;

        {
            let (stdout, _stderr) = git.run(&[
                "move",
                "-s",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ])?;
            insta::assert_snapshot!(stdout, @r###"
            Attempting rebase in-memory...
            Rebase in-memory (1/2): create test3.txt
            Rebase in-memory (2/2): create test4.txt
            branchless: processing 2 rewritten commits
            In-memory rebase succeeded.
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d2 create test1.txt
            |\
            | o 4838e49b create test3.txt
            | |
            | o a2482074 create test4.txt
            |
            O 96d1c37a (master) create test2.txt
            |
            x 70deb1e2 (rewritten as 4838e49b) create test3.txt
            |
            % 355e173b (rewritten as a2482074) create test4.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_move_tree_on_disk() -> anyhow::Result<()> {
    with_git(|git| {
        if has_git_v2_24_bug(&git)? {
            return Ok(());
        }

        git.init_repo()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;

        git.detach_head()?;
        let test3_oid = git.commit_file("test3", 3)?;
        git.commit_file("test4", 4)?;
        git.run(&["checkout", &test3_oid.to_string()])?;
        git.commit_file("test5", 5)?;

        git.run(&[
            "move",
            "--on-disk",
            "-s",
            &test3_oid.to_string(),
            "-d",
            &test1_oid.to_string(),
        ])?;
        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d2 create test1.txt
            |\
            | @ cade1d30 create test3.txt
            | |\
            | | o 5bb72580 create test4.txt
            | |
            | o df755ed1 create test5.txt
            |
            O 96d1c37a (master) create test2.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_move_tree_in_memory() -> anyhow::Result<()> {
    with_git(|git| {
        if has_git_v2_24_bug(&git)? {
            return Ok(());
        }

        git.init_repo()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;

        git.detach_head()?;
        let test3_oid = git.commit_file("test3", 3)?;
        git.commit_file("test4", 4)?;
        git.run(&["checkout", &test3_oid.to_string()])?;
        git.commit_file("test5", 5)?;

        git.run(&[
            "move",
            "-s",
            &test3_oid.to_string(),
            "-d",
            &test1_oid.to_string(),
        ])?;
        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d2 create test1.txt
            |\
            | o 4838e49b create test3.txt
            | |\
            | | o a2482074 create test4.txt
            | |
            | o b1f9efa0 create test5.txt
            |
            O 96d1c37a (master) create test2.txt
            |
            x 70deb1e2 (rewritten as 4838e49b) create test3.txt
            |
            % 9ea1b368 (rewritten as b1f9efa0) create test5.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_move_with_source_not_in_smartlog_on_disk() -> anyhow::Result<()> {
    with_git(|git| {
        if has_git_v2_24_bug(&git)? {
            return Ok(());
        }

        git.init_repo()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;

        let test3_oid = git.commit_file("test3", 3)?;
        git.commit_file("test4", 4)?;

        git.run(&[
            "move",
            "--on-disk",
            "-s",
            &test3_oid.to_string(),
            "-d",
            &test1_oid.to_string(),
        ])?;
        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d2 create test1.txt
            |\
            : o 96d1c37a create test2.txt
            :
            @ 5bb72580 (master) create test4.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_move_with_source_not_in_smartlog_in_memory() -> anyhow::Result<()> {
    with_git(|git| {
        if has_git_v2_24_bug(&git)? {
            return Ok(());
        }

        git.init_repo()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;

        let test3_oid = git.commit_file("test3", 3)?;
        git.commit_file("test4", 4)?;

        git.run(&[
            "move",
            "-s",
            &test3_oid.to_string(),
            "-d",
            &test1_oid.to_string(),
        ])?;
        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d2 create test1.txt
            |\
            : o 4838e49b create test3.txt
            : |
            : o a2482074 create test4.txt
            :
            X 70deb1e2 (rewritten as 4838e49b) create test3.txt
            |
            % 355e173b (rewritten as a2482074) (master) create test4.txt
            "###);
        }

        Ok(())
    })
}
