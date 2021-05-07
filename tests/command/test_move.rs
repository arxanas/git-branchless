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
fn test_move_stick() -> anyhow::Result<()> {
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
fn test_move_tree() -> anyhow::Result<()> {
    with_git(|git| {
        if has_git_v2_24_bug(&git)? {
            return Ok(());
        }

        git.init_repo()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;

        // TODO: don't require that the source commit be shown in the smartlog.
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
fn test_move_with_source_not_in_smartlog() -> anyhow::Result<()> {
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
            : o 96d1c37a create test2.txt
            :
            @ 5bb72580 (master) create test4.txt
            "###);
        }

        Ok(())
    })
}
