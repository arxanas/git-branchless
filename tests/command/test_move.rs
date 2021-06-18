use branchless::{
    testing::{with_git, Git, GitRunOptions},
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
            branchless: processing 2 rewritten commits
            branchless: <git-executable> checkout a248207402822b7396cabe0f1011d8a7ce7daf1b
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
            | @ a2482074 create test4.txt
            |
            O 96d1c37a (master) create test2.txt
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
            | @ b1f9efa0 create test5.txt
            |
            O 96d1c37a (master) create test2.txt
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
            : @ a2482074 create test4.txt
            :
            X 70deb1e2 (rewritten as 4838e49b) create test3.txt
            |
            X 355e173b (rewritten as a2482074) (master) create test4.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_move_merge_conflict() -> anyhow::Result<()> {
    with_git(|git| {
        if has_git_v2_24_bug(&git)? {
            return Ok(());
        }

        git.init_repo()?;
        let base_oid = git.commit_file("test1", 1)?;
        git.detach_head()?;
        let other_oid = git.commit_file_with_contents("conflict", 2, "conflict 1\n")?;
        git.run(&["checkout", &base_oid.to_string()])?;
        git.commit_file_with_contents("conflict", 2, "conflict 2\n")?;

        {
            let (stdout, _stderr) = git.run_with_options(
                &["move", "-s", &other_oid.to_string()],
                &GitRunOptions {
                    expected_exit_code: 1,
                    ..Default::default()
                },
            )?;
            insta::assert_snapshot!(stdout, @r###"
            Attempting rebase in-memory...
            Merge conflict, falling back to rebase on-disk. The conflicting commit was: e85d25c7 create conflict.txt
            branchless: <git-executable> rebase --continue
            CONFLICT (add/add): Merge conflict in conflict.txt
            Auto-merging conflict.txt
            "###);
        }

        git.resolve_file("conflict", "resolved")?;
        {
            let (stdout, _stderr) = git.run(&["rebase", "--continue"])?;
            insta::assert_snapshot!(stdout, @r###"
            [detached HEAD 244e2bd] create conflict.txt
             1 file changed, 1 insertion(+), 1 deletion(-)
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d2 (master) create test1.txt
            |
            o 202143f2 create conflict.txt
            |
            @ 244e2bd1 create conflict.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_move_base() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.commit_file("test1", 1)?;
        git.detach_head()?;
        git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        git.run(&["checkout", "master"])?;
        git.commit_file("test4", 4)?;

        {
            let (stdout, _stderr) = git.run(&["move", "--base", &test3_oid.to_string()])?;
            insta::assert_snapshot!(stdout, @r###"
            Attempting rebase in-memory...
            branchless: processing 2 rewritten commits
            In-memory rebase succeeded.
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            :
            @ bf0d52a6 (master) create test4.txt
            |
            o 44352d00 create test2.txt
            |
            o cf5eb244 create test3.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_move_checkout_new_head() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.commit_file("test1", 1)?;
        git.run(&["prev"])?;
        git.commit_file("test2", 2)?;

        {
            let (stdout, _stderr) = git.run(&["move", "-d", "master"])?;
            insta::assert_snapshot!(stdout, @r###"
            Attempting rebase in-memory...
            branchless: processing 1 rewritten commit
            branchless: <git-executable> checkout 96d1c37a3d4363611c49f7e52186e189a04c531f
            In-memory rebase succeeded.
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d2 (master) create test1.txt
            |
            @ 96d1c37a create test2.txt
            "###);
        }

        Ok(())
    })
}

// TODO: implement restack in terms of move
// TODO: if on a rewritten commit before rebase, check out the new commit afterwards.
// TODO: move branches after in-memory rebase. Make sure to call reference-transaction hook.
// TODO: don't re-apply already-applied commits
