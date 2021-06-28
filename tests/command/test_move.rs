use branchless::testing::{make_git, GitRunOptions};

#[test]
fn test_move_stick_on_disk() -> anyhow::Result<()> {
    let git = make_git()?;

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
}

#[test]
fn test_move_stick_in_memory() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

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
}

#[test]
fn test_move_tree_on_disk() -> anyhow::Result<()> {
    let git = make_git()?;

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
}

#[test]
fn test_move_tree_in_memory() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

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
}

#[test]
fn test_move_with_source_not_in_smartlog_on_disk() -> anyhow::Result<()> {
    let git = make_git()?;

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
}

#[test]
fn test_move_with_source_not_in_smartlog_in_memory() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

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
            branchless: processing 1 update to a branch/ref
            branchless: processing 2 rewritten commits
            branchless: <git-executable> checkout master
            In-memory rebase succeeded.
            "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d2 create test1.txt
        |\
        : o 96d1c37a create test2.txt
        :
        @ a2482074 (master) create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_merge_conflict() -> anyhow::Result<()> {
    let git = make_git()?;

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
}

#[test]
fn test_move_base() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

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
}

#[test]
fn test_move_checkout_new_head() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

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
}

#[test]
fn test_move_branch() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["checkout", "master"])?;

    git.commit_file("test3", 3)?;

    {
        let (stdout, _stderr) = git.run(&["move", "-d", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
            Attempting rebase in-memory...
            branchless: processing 1 update to a branch/ref
            branchless: processing 1 rewritten commit
            branchless: <git-executable> checkout master
            In-memory rebase succeeded.
            "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
            :
            @ 70deb1e2 (master) create test3.txt
            "###);
    }

    {
        let (stdout, _stderr) = git.run(&["branch", "--show-current"])?;
        assert_eq!(stdout, "master\n");
    }

    Ok(())
}

#[test]
fn test_move_base_onto_head() -> anyhow::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    {
        // Codifying current behavior -- we attempt to apply the commits again,
        // which is probably not intuitive.
        let (stdout, stderr) = git.run(&["move", "-b", "HEAD"])?;
        insta::assert_snapshot!(stderr, @r###"
        Previous HEAD position was 70deb1e create test3.txt
        branchless: processing 1 update to a branch/ref
        HEAD is now at a45568b create test3.txt
        branchless: processing checkout
        "###);
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        branchless: processing 2 rewritten commits
        branchless: This operation abandoned 1 commit!
        branchless: Consider running one of the following:
        branchless:   - git restack: re-apply the abandoned commits/branches
        branchless:     (this is most likely what you want to do)
        branchless:   - git smartlog: assess the situation
        branchless:   - git hide [<commit>...]: hide the commits from the smartlog
        branchless:   - git undo: undo the operation
        branchless:   - git config branchless.restack.warnAbandoned false: suppress this message
        branchless: <git-executable> checkout a45568bde9ac0b74d3bc890d11cacc789dc15294
        In-memory rebase succeeded.
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d2 (master) create test1.txt
        |
        x 96d1c37a (rewritten as 5e95ed7c) create test2.txt
        |
        x 70deb1e2 (rewritten as a45568bd) create test3.txt
        |
        o 5e95ed7c create test2.txt
        |
        @ a45568bd create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_force_in_memory() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD~"])?;

    git.write_file("test2", "conflicting contents")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "conflicting test2"])?;

    {
        let (stdout, stderr) = git.run_with_options(
            &["move", "-d", "master", "--in-memory"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        Merge conflict. The conflicting commit was: 081b474b conflicting test2
        Aborting since an in-memory rebase was requested.
        "###);
    }

    Ok(())
}

#[test]
fn test_rebase_in_memory_updates_committer_timestamp() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    let repo = git.get_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.detach_head()?;
    git.run(&["checkout", "HEAD~"])?;
    git.commit_file("test3", 3)?;

    let original_committer_timestamp = repo.head()?.peel_to_commit()?.committer().when();
    git.run(&["move", "-d", "master"])?;
    let updated_committer_timestamp = repo.head()?.peel_to_commit()?.committer().when();

    println!(
        "original_committer_timestamp: {:?} {:?}",
        original_committer_timestamp.seconds(),
        original_committer_timestamp.offset_minutes()
    );
    println!(
        "updated_committer_timestamp: {:?} {:?}",
        updated_committer_timestamp.seconds(),
        updated_committer_timestamp.offset_minutes()
    );
    assert!(original_committer_timestamp < updated_committer_timestamp);

    Ok(())
}

// TODO: implement restack in terms of move
// TODO: don't re-apply already-applied commits
