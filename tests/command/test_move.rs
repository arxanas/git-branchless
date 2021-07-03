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
            "--debug-dump-rebase-plan",
            "-s",
            &test3_oid.to_string(),
            "-d",
            &test1_oid.to_string(),
        ])?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                commands: [
                    ResetToOid {
                        commit_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                    },
                    Pick {
                        commit_oid: 70deb1e28791d8e7dd5a1f0c871a51b91282562f,
                    },
                    Pick {
                        commit_oid: 355e173bf9c5d2efac2e451da0cdad3fb82b869a,
                    },
                ],
            },
        )
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
            "--debug-dump-rebase-plan",
            "-s",
            &test3_oid.to_string(),
            "-d",
            &test1_oid.to_string(),
        ])?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                commands: [
                    ResetToOid {
                        commit_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                    },
                    Pick {
                        commit_oid: 70deb1e28791d8e7dd5a1f0c871a51b91282562f,
                    },
                    Pick {
                        commit_oid: 355e173bf9c5d2efac2e451da0cdad3fb82b869a,
                    },
                ],
            },
        )
        Attempting rebase in-memory...
        branchless: processing 1 update: branch master
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
            &[
                "move",
                "--debug-dump-rebase-plan",
                "-s",
                &other_oid.to_string(),
            ],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: 202143f2fdfc785285ab097422f6a695ff1d93cb,
                commands: [
                    ResetToOid {
                        commit_oid: 202143f2fdfc785285ab097422f6a695ff1d93cb,
                    },
                    Pick {
                        commit_oid: e85d25c772a05b5c73ea8ec43881c12bbf588848,
                    },
                ],
            },
        )
        Attempting rebase in-memory...
        Merge conflict, falling back to rebase on-disk. The conflicting commit was: e85d25c7 create conflict.txt
        Calling Git for on-disk rebase...
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
        let (stdout, _stderr) = git.run(&[
            "move",
            "--debug-dump-rebase-plan",
            "--base",
            &test3_oid.to_string(),
        ])?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: bf0d52a607f693201512a43b6b5a70b2a275e0ad,
                commands: [
                    ResetToOid {
                        commit_oid: bf0d52a607f693201512a43b6b5a70b2a275e0ad,
                    },
                    Pick {
                        commit_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                    },
                    Pick {
                        commit_oid: 70deb1e28791d8e7dd5a1f0c871a51b91282562f,
                    },
                ],
            },
        )
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
        let (stdout, _stderr) = git.run(&["move", "--debug-dump-rebase-plan", "-d", "master"])?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                commands: [
                    ResetToOid {
                        commit_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                    },
                    Pick {
                        commit_oid: fe65c1fe15584744e649b2c79d4cf9b0d878f92e,
                    },
                ],
            },
        )
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
        let (stdout, _stderr) = git.run(&[
            "move",
            "--debug-dump-rebase-plan",
            "-d",
            &test2_oid.to_string(),
        ])?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                commands: [
                    ResetToOid {
                        commit_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                    },
                    Pick {
                        commit_oid: 98b9119d16974f372e76cb64a3b77c528fc0b18b,
                    },
                ],
            },
        )
        Attempting rebase in-memory...
        branchless: processing 1 update: branch master
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
        let (stdout, stderr) = git.run_with_options(
            &["move", "--debug-dump-rebase-plan", "-b", "HEAD"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        This operation failed because it would introduce a cycle:
        ,-> 70deb1e2 create test3.txt
        |   96d1c37a create test2.txt
        `-- 70deb1e2 create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d2 (master) create test1.txt
        |
        o 96d1c37a create test2.txt
        |
        @ 70deb1e2 create test3.txt
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

    let original_committer_timestamp = repo
        .find_commit(repo.get_head_info()?.oid.unwrap())?
        .unwrap()
        .get_committer()
        .get_time();
    git.run(&["move", "-d", "master"])?;
    let updated_committer_timestamp = repo
        .find_commit(repo.get_head_info()?.oid.unwrap())?
        .unwrap()
        .get_committer()
        .get_time();

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

#[test]
fn test_move_in_memory_gc() -> anyhow::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, stderr) = git.run(&[
            "move",
            "--debug-dump-rebase-plan",
            "-d",
            "master",
            "--in-memory",
        ])?;
        insta::assert_snapshot!(stderr, @r###"
        Previous HEAD position was 96d1c37 create test2.txt
        branchless: processing 1 update: ref HEAD
        HEAD is now at fe65c1f create test2.txt
        branchless: processing checkout
        "###);
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                commands: [
                    ResetToOid {
                        commit_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                    },
                    Pick {
                        commit_oid: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                    },
                ],
            },
        )
        Attempting rebase in-memory...
        branchless: processing 1 rewritten commit
        branchless: <git-executable> checkout fe65c1fe15584744e649b2c79d4cf9b0d878f92e
        In-memory rebase succeeded.
        "###);
    }

    git.run(&["checkout", &test1_oid.to_string()])?;

    {
        let (stdout, stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 (master) create initial.txt
        |\
        | @ 62fc20d2 create test1.txt
        |
        o fe65c1fe create test2.txt
        "###);
    }

    git.run(&["gc", "--prune=now"])?;

    {
        let (stdout, stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 (master) create initial.txt
        |\
        | @ 62fc20d2 create test1.txt
        |
        o fe65c1fe create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_main_branch_commits() -> anyhow::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;

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
        branchless: processing 1 update: branch master
        branchless: processing 3 rewritten commits
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
        @ 566e4341 (master) create test5.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["log"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit 566e4341a4a9a930fc2bf7ccdfa168e9f266c34a
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0500

            create test5.txt

        commit a248207402822b7396cabe0f1011d8a7ce7daf1b
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0400

            create test4.txt

        commit 4838e49b08954becdd17c0900c1179c2c654c627
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0300

            create test3.txt

        commit 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0100

            create test1.txt

        commit f777ecc9b0db5ed372b2615695191a8a17f79f24
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            create initial.txt
        "###);
    }

    Ok(())
}

// TODO: don't re-apply already-applied commits
