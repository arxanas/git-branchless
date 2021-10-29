use branchless::testing::{
    make_git, make_git_with_remote_repo, GitInitOptions, GitRunOptions, GitWrapperWithRemoteRepo,
};

use crate::command::test_restack::remove_rebase_lines;

#[test]
fn test_move_stick() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;

    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    git.detach_head()?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.run(&[
            "move",
            "--on-disk",
            "-s",
            &test3_oid.to_string(),
            "-d",
            &test1_oid.to_string(),
        ])?;

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

    // --in-memory
    {
        let (stdout, _stderr) = git.run(&[
            "move",
            "--in-memory",
            "--debug-dump-rebase-plan",
            "-s",
            &test3_oid.to_string(),
            "-d",
            &test1_oid.to_string(),
        ])?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                commands: [
                    RegisterExtraPostRewriteHook,
                    Reset {
                        target: Oid(
                            NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                        ),
                    },
                    Pick {
                        commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                    },
                    Pick {
                        commit_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                    },
                ],
            },
        )
        Attempting rebase in-memory...
        [1/2] Committed as: 4838e49b create test3.txt
        [2/2] Committed as: a2482074 create test4.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout a248207402822b7396cabe0f1011d8a7ce7daf1b
        In-memory rebase succeeded.
        "###);

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
fn test_move_tree() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
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

    // --on-disk
    {
        let git = git.duplicate_repo()?;
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
        | o 4838e49b create test3.txt
        | |\
        | | o a2482074 create test4.txt
        | |
        | @ b1f9efa0 create test5.txt
        |
        O 96d1c37a (master) create test2.txt
        "###);
        }
    }

    // in-memory
    {
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
    }

    Ok(())
}

#[test]
fn test_move_with_source_not_in_smartlog() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;

    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;

    // on-disk
    {
        let git = git.duplicate_repo()?;

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
        @ a2482074 (master) create test4.txt
        "###);
        }
    }

    // --in-memory
    {
        {
            let (stdout, _stderr) = git.run(&[
                "move",
                "--in-memory",
                "--debug-dump-rebase-plan",
                "-s",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ])?;
            insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                commands: [
                    RegisterExtraPostRewriteHook,
                    Reset {
                        target: Oid(
                            NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                        ),
                    },
                    Pick {
                        commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                    },
                    Pick {
                        commit_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                    },
                ],
            },
        )
        Attempting rebase in-memory...
        [1/2] Committed as: 4838e49b create test3.txt
        [2/2] Committed as: a2482074 create test4.txt
        branchless: processing 1 update: branch master
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout master
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
    }

    Ok(())
}

#[test]
fn test_move_merge_conflict() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
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
            &["move", "--source", &other_oid.to_string()],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        This operation would cause a merge conflict:
        - (1 conflicting file) e85d25c7 create conflict.txt
        To resolve merge conflicts, retry this operation with the --merge option.
        "###);
    }

    {
        let (stdout, _stderr) = git.run_with_options(
            &[
                "move",
                "--debug-dump-rebase-plan",
                "--merge",
                "--source",
                &other_oid.to_string(),
            ],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        let stdout = remove_rebase_lines(stdout);

        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: NonZeroOid(202143f2fdfc785285ab097422f6a695ff1d93cb),
                commands: [
                    RegisterExtraPostRewriteHook,
                    Reset {
                        target: Oid(
                            NonZeroOid(202143f2fdfc785285ab097422f6a695ff1d93cb),
                        ),
                    },
                    Pick {
                        commit_oid: NonZeroOid(e85d25c772a05b5c73ea8ec43881c12bbf588848),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(e85d25c772a05b5c73ea8ec43881c12bbf588848),
                    },
                ],
            },
        )
        Attempting rebase in-memory...
        There was a merge conflict, which currently can't be resolved when rebasing in-memory.
        The conflicting commit was: e85d25c7 create conflict.txt
        Trying again on-disk...
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        CONFLICT (add/add): Merge conflict in conflict.txt
        "###);
    }

    git.resolve_file("conflict", "resolved")?;
    {
        let (stdout, _stderr) = git.run(&["rebase", "--continue"])?;
        insta::assert_snapshot!(stdout, @r###"
        [detached HEAD 42951b5] create conflict.txt
         1 file changed, 1 insertion(+), 1 deletion(-)
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d2 (master) create test1.txt
        |
        @ 202143f2 create conflict.txt
        |
        o 42951b5f create conflict.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_base() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

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
                first_dest_oid: NonZeroOid(bf0d52a607f693201512a43b6b5a70b2a275e0ad),
                commands: [
                    RegisterExtraPostRewriteHook,
                    Reset {
                        target: Oid(
                            NonZeroOid(bf0d52a607f693201512a43b6b5a70b2a275e0ad),
                        ),
                    },
                    Pick {
                        commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                    },
                    Pick {
                        commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                    },
                ],
            },
        )
        Attempting rebase in-memory...
        [1/2] Committed as: 44352d00 create test2.txt
        [2/2] Committed as: cf5eb244 create test3.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout master
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
fn test_move_base_shared() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD~"])?;
    git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;

    {
        let (stdout, stderr) = git.run(&["move", "-b", "HEAD", "-d", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stderr, @r###"
        Previous HEAD position was a248207 create test4.txt
        branchless: processing 1 update: ref HEAD
        HEAD is now at 355e173 create test4.txt
        branchless: processing checkout
        "###);
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/2] Committed as: 70deb1e2 create test3.txt
        [2/2] Committed as: 355e173b create test4.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout 355e173bf9c5d2efac2e451da0cdad3fb82b869a
        In-memory rebase succeeded.
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        o 96d1c37a create test2.txt
        |
        o 70deb1e2 create test3.txt
        |
        @ 355e173b create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_checkout_new_head() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.run(&["prev"])?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = git.run(&["move", "--debug-dump-rebase-plan", "-d", "master"])?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                commands: [
                    RegisterExtraPostRewriteHook,
                    Reset {
                        target: Oid(
                            NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                        ),
                    },
                    Pick {
                        commit_oid: NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                    },
                ],
            },
        )
        Attempting rebase in-memory...
        [1/1] Committed as: 96d1c37a create test2.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout 96d1c37a3d4363611c49f7e52186e189a04c531f
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
fn test_move_branch() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

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
                first_dest_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                commands: [
                    RegisterExtraPostRewriteHook,
                    Reset {
                        target: Oid(
                            NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                        ),
                    },
                    Pick {
                        commit_oid: NonZeroOid(98b9119d16974f372e76cb64a3b77c528fc0b18b),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(98b9119d16974f372e76cb64a3b77c528fc0b18b),
                    },
                ],
            },
        )
        Attempting rebase in-memory...
        [1/1] Committed as: 70deb1e2 create test3.txt
        branchless: processing 1 update: branch master
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout master
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
fn test_move_base_onto_head() -> eyre::Result<()> {
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
            &["move", "--debug-dump-rebase-plan", "-b", "HEAD^"],
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
fn test_move_force_in_memory() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

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
        There was a merge conflict, which currently can't be resolved when rebasing in-memory.
        The conflicting commit was: 081b474b conflicting test2
        Aborting since an in-memory rebase was requested.
        "###);
    }

    Ok(())
}

#[test]
fn test_rebase_in_memory_updates_committer_timestamp() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "false"])?;

    let repo = git.get_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.detach_head()?;
    git.run(&["checkout", "HEAD~"])?;
    git.commit_file("test3", 3)?;

    let original_committer_timestamp = repo
        .find_commit_or_fail(repo.get_head_info()?.oid.unwrap())?
        .get_committer()
        .get_time();
    git.run(&["move", "-d", "master"])?;
    let updated_committer_timestamp = repo
        .find_commit_or_fail(repo.get_head_info()?.oid.unwrap())?
        .get_committer()
        .get_time();

    assert!(original_committer_timestamp < updated_committer_timestamp);

    Ok(())
}

#[test]
fn test_move_in_memory_gc() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;

    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, stderr) = git.run(&[
            "move",
            "--debug-dump-rebase-plan",
            "-s",
            "HEAD",
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
                first_dest_oid: NonZeroOid(f777ecc9b0db5ed372b2615695191a8a17f79f24),
                commands: [
                    RegisterExtraPostRewriteHook,
                    Reset {
                        target: Oid(
                            NonZeroOid(f777ecc9b0db5ed372b2615695191a8a17f79f24),
                        ),
                    },
                    Pick {
                        commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                    },
                ],
            },
        )
        Attempting rebase in-memory...
        [1/1] Committed as: fe65c1fe create test2.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout fe65c1fe15584744e649b2c79d4cf9b0d878f92e
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
fn test_move_main_branch_commits() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;

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
        [1/3] Committed as: 4838e49b create test3.txt
        [2/3] Committed as: a2482074 create test4.txt
        [3/3] Committed as: 566e4341 create test5.txt
        branchless: processing 1 update: branch master
        branchless: processing 3 rewritten commits
        branchless: running command: <git-executable> checkout master
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

#[test]
fn test_move_branches_after_move() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.detach_head()?;
    git.commit_file("test3", 3)?;
    git.run(&["branch", "foo"])?;
    git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;
    git.run(&["branch", "bar"])?;

    // --on-disk
    {
        let git = git.duplicate_repo()?;

        {
            let (stdout, stderr) = git.run(&[
                "move",
                "--on-disk",
                "-s",
                "foo",
                "-d",
                &test1_oid.to_string(),
            ])?;
            insta::assert_snapshot!(stderr, @r###"
        Executing: git branchless hook-register-extra-post-rewrite-hook
        branchless: processing 1 update: ref HEAD
        branchless: processing 1 update: ref HEAD
        branchless: processed commit: 4838e49b create test3.txt
        Executing: git branchless hook-detect-empty-commit 70deb1e28791d8e7dd5a1f0c871a51b91282562f
        branchless: processing 1 update: ref HEAD
        branchless: processed commit: a2482074 create test4.txt
        Executing: git branchless hook-detect-empty-commit 355e173bf9c5d2efac2e451da0cdad3fb82b869a
        branchless: processing 1 update: ref HEAD
        branchless: processed commit: 566e4341 create test5.txt
        Executing: git branchless hook-detect-empty-commit f81d55c0d520ff8d02ef9294d95156dcb78a5255
        branchless: processing 3 rewritten commits
        branchless: processing 2 updates: branch bar, branch foo
        Successfully rebased and updated detached HEAD.
        "###);
            insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d2 create test1.txt
        |\
        | o 4838e49b (foo) create test3.txt
        | |
        | o a2482074 create test4.txt
        | |
        | @ 566e4341 (bar) create test5.txt
        |
        O 96d1c37a (master) create test2.txt
        "###);
        }

        {
            // There should be no branches left to restack.
            let (stdout, _stderr) = git.run(&["restack"])?;
            insta::assert_snapshot!(stdout, @r###"
        No abandoned commits to restack.
        No abandoned branches to restack.
        branchless: running command: <git-executable> checkout 566e4341a4a9a930fc2bf7ccdfa168e9f266c34a
        :
        O 62fc20d2 create test1.txt
        |\
        | o 4838e49b (foo) create test3.txt
        | |
        | o a2482074 create test4.txt
        | |
        | @ 566e4341 (bar) create test5.txt
        |
        O 96d1c37a (master) create test2.txt
        "###);
        }
    }

    {
        {
            let (stdout, stderr) = git.run(&[
                "move",
                "--in-memory",
                "-s",
                "foo",
                "-d",
                &test1_oid.to_string(),
            ])?;
            insta::assert_snapshot!(stderr, @r###"
            Previous HEAD position was f81d55c create test5.txt
            branchless: processing 1 update: ref HEAD
            HEAD is now at 566e434 create test5.txt
            branchless: processing checkout
            "###);
            insta::assert_snapshot!(stdout, @r###"
            Attempting rebase in-memory...
            [1/3] Committed as: 4838e49b create test3.txt
            [2/3] Committed as: a2482074 create test4.txt
            [3/3] Committed as: 566e4341 create test5.txt
            branchless: processing 2 updates: branch bar, branch foo
            branchless: processing 3 rewritten commits
            branchless: running command: <git-executable> checkout 566e4341a4a9a930fc2bf7ccdfa168e9f266c34a
            In-memory rebase succeeded.
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d2 create test1.txt
            |\
            | o 4838e49b (foo) create test3.txt
            | |
            | o a2482074 create test4.txt
            | |
            | @ 566e4341 (bar) create test5.txt
            |
            O 96d1c37a (master) create test2.txt
            "###);
        }

        {
            // There should be no branches left to restack.
            let (stdout, _stderr) = git.run(&["restack"])?;
            insta::assert_snapshot!(stdout, @r###"
        No abandoned commits to restack.
        No abandoned branches to restack.
        branchless: running command: <git-executable> checkout 566e4341a4a9a930fc2bf7ccdfa168e9f266c34a
        :
        O 62fc20d2 create test1.txt
        |\
        | o 4838e49b (foo) create test3.txt
        | |
        | o a2482074 create test4.txt
        | |
        | @ 566e4341 (bar) create test5.txt
        |
        O 96d1c37a (master) create test2.txt
        "###);
        }
    }

    Ok(())
}

#[test]
fn test_move_no_reapply_upstream_commits() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;

    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.run(&["branch", "should-be-deleted"])?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["checkout", "master"])?;
    git.run(&["cherry-pick", &test1_oid.to_string()])?;
    git.run(&["checkout", &test2_oid.to_string()])?;

    // --on-disk
    {
        let git = git.duplicate_repo()?;

        {
            let (stdout, stderr) = git.run(&["move", "--on-disk", "-b", "HEAD", "-d", "master"])?;
            insta::assert_snapshot!(stderr, @r###"
            Executing: git branchless hook-register-extra-post-rewrite-hook
            branchless: processing 1 update: ref HEAD
            Executing: git branchless hook-skip-upstream-applied-commit 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: fa466332 create test2.txt
            Executing: git branchless hook-detect-empty-commit 96d1c37a3d4363611c49f7e52186e189a04c531f
            branchless: processing 2 rewritten commits
            branchless: processing 1 update: branch should-be-deleted
            Successfully rebased and updated detached HEAD.
            "###);
            insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Skipping commit (was already applied upstream): 62fc20d2 create test1.txt
        "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 047b7ad7 (master) create test1.txt
            |
            @ fa466332 create test2.txt
            "###);
        }
    }

    // --in-memory
    {
        {
            let (stdout, stderr) =
                git.run(&["move", "--in-memory", "-b", "HEAD", "-d", "master"])?;
            insta::assert_snapshot!(stderr, @r###"
        Previous HEAD position was 96d1c37 create test2.txt
        branchless: processing 1 update: ref HEAD
        HEAD is now at fa46633 create test2.txt
        branchless: processing checkout
        "###);
            insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/2] Skipped commit (was already applied upstream): 62fc20d2 create test1.txt
        [2/2] Committed as: fa466332 create test2.txt
        branchless: processing 1 update: branch should-be-deleted
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout fa46633239bfa767036e41a77b67258286e4ddb9
        In-memory rebase succeeded.
        "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
        :
        O 047b7ad7 (master) create test1.txt
        |
        @ fa466332 create test2.txt
        "###);
        }
    }
    Ok(())
}

#[test]
fn test_move_no_reapply_squashed_commits() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;

    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;

    git.run(&["checkout", "master"])?;
    git.run(&["cherry-pick", "--no-commit", &test1_oid.to_string()])?;
    git.run(&["cherry-pick", "--no-commit", &test2_oid.to_string()])?;
    git.run(&["commit", "-m", "squashed test1 and test2"])?;

    // --on-disk
    {
        let git = git.duplicate_repo()?;

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 create initial.txt
        |\
        | o 62fc20d2 create test1.txt
        | |
        | o 96d1c37a create test2.txt
        |
        @ de4a1fe8 (master) squashed test1 and test2
        "###);
        }

        {
            let (stdout, stderr) = git.run(&[
                "move",
                "--on-disk",
                "-b",
                &test2_oid.to_string(),
                "-d",
                "master",
            ])?;
            insta::assert_snapshot!(stderr, @r###"
            Executing: git branchless hook-register-extra-post-rewrite-hook
            branchless: processing 1 update: ref HEAD
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: e7bcdd60 create test1.txt
            Executing: git branchless hook-detect-empty-commit 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: 12d361aa create test2.txt
            Executing: git branchless hook-detect-empty-commit 96d1c37a3d4363611c49f7e52186e189a04c531f
            branchless: processing 1 update: branch master
            branchless: processing 4 rewritten commits
            branchless: running command: <git-executable> checkout de4a1fe8f80b830d7d9a5b4adfd79fab3fcdc80c
            branchless: processing 1 update: ref HEAD
            HEAD is now at de4a1fe squashed test1 and test2
            branchless: processing checkout
            Successfully rebased and updated refs/heads/master.
            "###);
            insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Skipped now-empty commit: e7bcdd60 create test1.txt
        Skipped now-empty commit: 12d361aa create test2.txt
        "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
        :
        @ de4a1fe8 (master) squashed test1 and test2
        "###);
        }
    }

    // --in-memory
    {
        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 create initial.txt
        |\
        | o 62fc20d2 create test1.txt
        | |
        | o 96d1c37a create test2.txt
        |
        @ de4a1fe8 (master) squashed test1 and test2
        "###);
        }

        {
            let (stdout, stderr) = git.run(&[
                "move",
                "--in-memory",
                "-b",
                &test2_oid.to_string(),
                "-d",
                "master",
            ])?;
            insta::assert_snapshot!(stderr, @r###"
        Switched to branch 'master'
        branchless: processing checkout
        "###);
            insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/2] Skipped now-empty commit: e7bcdd60 create test1.txt
        [2/2] Skipped now-empty commit: 12d361aa create test2.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout master
        In-memory rebase succeeded.
        "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
        :
        @ de4a1fe8 (master) squashed test1 and test2
        "###);
        }
    }
    Ok(())
}

#[test]
fn test_move_delete_checked_out_branch() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;

    git.run(&["checkout", "-b", "work"])?;
    let test1_oid = git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["checkout", "-b", "more-work"])?;
    git.commit_file("test3", 2)?;
    git.run(&["checkout", "master"])?;
    git.run(&[
        "cherry-pick",
        &test1_oid.to_string(),
        &test2_oid.to_string(),
    ])?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 create initial.txt
        |\
        : o 62fc20d2 create test1.txt
        : |
        : o 96d1c37a (work) create test2.txt
        : |
        : o ffcba554 (more-work) create test3.txt
        :
        @ 91c5ce63 (master) create test2.txt
        "###);
    }

    // --on-disk
    {
        let git = git.duplicate_repo()?;

        {
            git.run(&["checkout", "work"])?;
            let (stdout, stderr) = git.run(&["move", "--on-disk", "-b", "HEAD", "-d", "master"])?;
            insta::assert_snapshot!(stderr, @r###"
            Executing: git branchless hook-register-extra-post-rewrite-hook
            branchless: processing 1 update: ref HEAD
            Executing: git branchless hook-skip-upstream-applied-commit 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
            Executing: git branchless hook-skip-upstream-applied-commit 96d1c37a3d4363611c49f7e52186e189a04c531f
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: 012efd6e create test3.txt
            Executing: git branchless hook-detect-empty-commit ffcba554683d83de283de084a7d3896e332bbcdb
            branchless: processing 3 rewritten commits
            branchless: processing 2 updates: branch more-work, branch work
            branchless: running command: <git-executable> checkout 91c5ce63686889388daec1120bf57bea8a744bc2
            Previous HEAD position was 012efd6 create test3.txt
            branchless: processing 1 update: ref HEAD
            HEAD is now at 91c5ce6 create test2.txt
            branchless: processing checkout
            Successfully rebased and updated detached HEAD.
            "###);
            insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Skipping commit (was already applied upstream): 62fc20d2 create test1.txt
        Skipping commit (was already applied upstream): 96d1c37a create test2.txt
        "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
        :
        @ 91c5ce63 (master) create test2.txt
        |
        o 012efd6e (more-work) create test3.txt
        "###);
        }
    }
    // --in-memory
    {
        {
            git.run(&["checkout", "work"])?;
            let (stdout, stderr) =
                git.run(&["move", "--in-memory", "-b", "HEAD", "-d", "master"])?;
            insta::assert_snapshot!(stderr, @r###"
        Previous HEAD position was 96d1c37 create test2.txt
        branchless: processing 1 update: ref HEAD
        HEAD is now at 91c5ce6 create test2.txt
        branchless: processing checkout
        "###);
            insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/3] Skipped commit (was already applied upstream): 62fc20d2 create test1.txt
        [2/3] Skipped commit (was already applied upstream): 96d1c37a create test2.txt
        [3/3] Committed as: 012efd6e create test3.txt
        branchless: processing 2 updates: branch more-work, branch work
        branchless: processing 3 rewritten commits
        branchless: running command: <git-executable> checkout 91c5ce63686889388daec1120bf57bea8a744bc2
        In-memory rebase succeeded.
        "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
        :
        @ 91c5ce63 (master) create test2.txt
        |
        o 012efd6e (more-work) create test3.txt
        "###);
        }
    }

    Ok(())
}

#[test]
fn test_move_with_unstaged_changes() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD~"])?;
    git.commit_file("test3", 3)?;

    {
        git.write_file("test3", "new contents")?;
        let (stdout, stderr) = git.run_with_options(
            &["move", "--on-disk", "-d", "master"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        This operation would modify the working copy, but you have uncommitted changes
        in your working copy which might be overwritten as a result.
        Commit your changes and then try again.
        "###);
    }

    Ok(())
}

#[test]
fn test_move_merge_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD^"])?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD^"])?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.run(&["merge", &test2_oid.to_string()])?;

    git.run(&["checkout", &test3_oid.to_string()])?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 create initial.txt
        |\
        | o fe65c1fe create test2.txt
        | |
        | o 28790c73 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
        |\
        | @ 98b9119d create test3.txt
        | |
        | o 28790c73 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
        |
        O 62fc20d2 (master) create test1.txt
        "###);
    }

    // --on-disk
    {
        let git = git.duplicate_repo()?;

        {
            let (stdout, stderr) = git.run(&[
                "move",
                "--debug-dump-rebase-constraints",
                "--debug-dump-rebase-plan",
                "--on-disk",
                "-s",
                &test2_oid.to_string(),
                "-d",
                "master",
            ])?;
            insta::assert_snapshot!(stdout, @r###"
        Rebase constraints before adding descendants: [
            (
                NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                [
                    NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                ],
            ),
        ]
        Rebase constraints after adding descendants: [
            (
                NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                [
                    NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                ],
            ),
            (
                NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                [
                    NonZeroOid(28790c73f13f38ce0d3beb6cfeb2d818b32bcd09),
                ],
            ),
        ]
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                commands: [
                    RegisterExtraPostRewriteHook,
                    Reset {
                        target: Oid(
                            NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                        ),
                    },
                    Pick {
                        commit_oid: NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                    },
                    CreateLabel {
                        label_name: "merge-parent-4",
                    },
                    Merge {
                        commit_oid: NonZeroOid(28790c73f13f38ce0d3beb6cfeb2d818b32bcd09),
                        commits_to_merge: [
                            Oid(
                                NonZeroOid(98b9119d16974f372e76cb64a3b77c528fc0b18b),
                            ),
                        ],
                    },
                ],
            },
        )
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        "###);
            insta::assert_snapshot!(stderr, @r###"
        Executing: git branchless hook-register-extra-post-rewrite-hook
        branchless: processing 1 update: ref HEAD
        branchless: processing 1 update: ref HEAD
        branchless: processed commit: 96d1c37a create test2.txt
        Executing: git branchless hook-detect-empty-commit fe65c1fe15584744e649b2c79d4cf9b0d878f92e
        branchless: processing 1 update: ref refs/rewritten/merge-parent-4
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout 98b9119d16974f372e76cb64a3b77c528fc0b18b
        Previous HEAD position was 96a2c4b Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
        branchless: processing 1 update: ref HEAD
        HEAD is now at 98b9119 create test3.txt
        branchless: processing checkout
        Successfully rebased and updated detached HEAD.
        branchless: processing 1 update: ref refs/rewritten/merge-parent-4
        "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 create initial.txt
        |\
        | @ 98b9119d create test3.txt
        | |
        | o 96a2c4be Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
        |
        O 62fc20d2 (master) create test1.txt
        |
        o 96d1c37a create test2.txt
        |
        o 96a2c4be Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
        "###);
        }
    }

    // --in-memory
    {
        let git = git.duplicate_repo()?;

        {
            let (stdout, _stderr) = git.run_with_options(
                &[
                    "move",
                    "--in-memory",
                    "-s",
                    &test2_oid.to_string(),
                    "-d",
                    "master",
                ],
                &GitRunOptions {
                    expected_exit_code: 1,
                    ..Default::default()
                },
            )?;
            insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        Merge commits currently can't be rebased in-memory.
        The merge commit was: 28790c73 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
        Aborting since an in-memory rebase was requested.
        "###);
        }
    }

    // no flag
    {
        {
            let (stdout, stderr) =
                git.run(&["move", "-s", &test2_oid.to_string(), "-d", "master"])?;
            insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        Merge commits currently can't be rebased in-memory.
        The merge commit was: 28790c73 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
        Trying again on-disk...
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        "###);
            insta::assert_snapshot!(stderr, @r###"
        Executing: git branchless hook-register-extra-post-rewrite-hook
        branchless: processing 1 update: ref HEAD
        branchless: processing 1 update: ref HEAD
        branchless: processed commit: 96d1c37a create test2.txt
        Executing: git branchless hook-detect-empty-commit fe65c1fe15584744e649b2c79d4cf9b0d878f92e
        branchless: processing 1 update: ref refs/rewritten/merge-parent-4
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout 98b9119d16974f372e76cb64a3b77c528fc0b18b
        Previous HEAD position was 96a2c4b Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
        branchless: processing 1 update: ref HEAD
        HEAD is now at 98b9119 create test3.txt
        branchless: processing checkout
        Successfully rebased and updated detached HEAD.
        branchless: processing 1 update: ref refs/rewritten/merge-parent-4
        "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 create initial.txt
        |\
        | @ 98b9119d create test3.txt
        | |
        | o 96a2c4be Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
        |
        O 62fc20d2 (master) create test1.txt
        |
        o 96d1c37a create test2.txt
        |
        o 96a2c4be Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
        "###);
        }
    }

    Ok(())
}

#[test]
fn test_move_orphaned_root() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    git.detach_head()?;
    git.run(&["checkout", "--orphan", "new-root"])?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 96d1c37a (master) create test2.txt
        "###);
    }

    git.run(&["commit", "-m", "new root"])?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        // FIXME: the smartlog handling for unrelated roots is wrong. There
        // should be no relation between these two commits.
        insta::assert_snapshot!(stdout, @r###"
        @ da90168b (new-root) new root
        :
        O 96d1c37a (master) create test2.txt
        "###);
    }

    git.commit_file("test3", 3)?;

    // --on-disk
    {
        let git = git.duplicate_repo()?;

        {
            let (stdout, stderr) = git.run(&["move", "--on-disk", "-d", "master"])?;
            insta::assert_snapshot!(stderr, @r###"
            Executing: git branchless hook-register-extra-post-rewrite-hook
            branchless: processing 1 update: ref HEAD
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: 270b681e new root
            Executing: git branchless hook-detect-empty-commit da90168b4835f97f1a10bcc12833140056df9157
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: 70deb1e2 create test3.txt
            Executing: git branchless hook-detect-empty-commit fc09f3d9f0b7370dc38e761e3730a856dc5025c2
            branchless: processing 1 update: branch new-root
            branchless: processing 3 rewritten commits
            branchless: running command: <git-executable> checkout 70deb1e28791d8e7dd5a1f0c871a51b91282562f
            branchless: processing 1 update: ref HEAD
            HEAD is now at 70deb1e create test3.txt
            branchless: processing checkout
            Successfully rebased and updated refs/heads/new-root.
            "###);
            insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Skipped now-empty commit: 270b681e new root
        "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
        :
        O 96d1c37a (master) create test2.txt
        |
        @ 70deb1e2 (new-root) create test3.txt
        "###);
        }
    }

    // --in-memory
    {
        {
            let (stdout, stderr) = git.run(&["move", "--in-memory", "-d", "master"])?;
            insta::assert_snapshot!(stderr, @r###"
        Previous HEAD position was fc09f3d create test3.txt
        Switched to branch 'new-root'
        branchless: processing checkout
        "###);
            insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/2] Skipped now-empty commit: 270b681e new root
        [2/2] Committed as: 70deb1e2 create test3.txt
        branchless: processing 1 update: branch new-root
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout new-root
        In-memory rebase succeeded.
        "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
        :
        O 96d1c37a (master) create test2.txt
        |
        @ 70deb1e2 (new-root) create test3.txt
        "###);
        }
    }

    Ok(())
}

#[test]
fn test_move_no_extra_checkout() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = git.run(&["move", "--in-memory", "-s", "HEAD", "-d", "HEAD^"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 96d1c37a create test2.txt
        branchless: processing 1 rewritten commit
        In-memory rebase succeeded.
        "###);
    }

    {
        git.run(&["branch", "foo"])?;
        let (stdout, _stderr) = git.run(&["move", "--in-memory", "-s", "HEAD", "-d", "HEAD^"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 96d1c37a create test2.txt
        branchless: processing 1 update: branch foo
        branchless: processing 1 rewritten commit
        In-memory rebase succeeded.
        "###);
    }

    Ok(())
}

#[test]
fn test_move_dest_not_in_dag() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;

    {
        original_repo.init_repo()?;
        original_repo.commit_file("test1", 1)?;
        original_repo.commit_file("test2", 2)?;
        original_repo.run(&["checkout", "-b", "other-branch", "HEAD^"])?;
        original_repo.commit_file("test3", 3)?;

        original_repo.clone_repo_into(&cloned_repo, &["--branch", "other-branch"])?;
    }

    {
        cloned_repo.init_repo_with_options(&GitInitOptions {
            make_initial_commit: false,
            run_branchless_init: false,
            ..Default::default()
        })?;
        cloned_repo.run(&["branchless", "init", "--main-branch", "other-branch"])?;

        let (stdout, _stderr) = cloned_repo.run(&["move", "-d", "origin/master"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 70deb1e2 create test3.txt
        branchless: processing 2 updates: branch other-branch, remote branch origin/other-branch
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout other-branch
        Your branch is up to date with 'origin/other-branch'.
        In-memory rebase succeeded.
        "###);
    }

    Ok(())
}

#[test]
fn test_move_abort_rebase_check_out_old_branch() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;

    git.run(&["checkout", "-b", "original"])?;
    git.commit_file_with_contents("test2", 2, "test2 original contents")?;

    git.run(&["checkout", "-b", "conflicting", "master"])?;
    git.commit_file_with_contents("test2", 2, "test2 conflicting contents")?;

    {
        let (stdout, _stderr) = git.run_with_options(
            &["move", "-d", "original", "--on-disk"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        let stdout = remove_rebase_lines(stdout);

        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        CONFLICT (add/add): Merge conflict in test2.txt
        "###);
    }

    git.run(&["rebase", "--abort"])?;

    {
        // Will output `HEAD` if `HEAD` is detached, which is not what we want.
        let (stdout, _stderr) = git.run(&["rev-parse", "--abbrev-ref", "HEAD"])?;
        insta::assert_snapshot!(stdout, @"conflicting");
    }

    Ok(())
}

/// For https://github.com/arxanas/git-branchless/issues/151
#[test]
fn test_move_orig_head_no_symbolic_reference() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    git.run(&["checkout", "-b", "foo"])?;
    // Force `ORIG_HEAD` to be written to disk.
    git.run(&["move", "-d", "HEAD^", "--on-disk"])?;
    git.detach_head()?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 96d1c37a (foo, master) create test2.txt
        "###);
    }

    // Get `git reset` to write a new value to `ORIG_HEAD`. If `ORIG_HEAD` is a
    // symbolic reference, it will write to the target reference, rather than
    // overwriting `ORIG_HEAD`.
    git.run(&["reset", "--hard", "HEAD^"])?;
    git.run(&["reset", "--hard", "HEAD^"])?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        // `foo` should be unmoved here, rather than moved to
        // `create test1.txt`.
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc9 create initial.txt
        :
        O 96d1c37a (foo, master) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_standalone_no_create_gc_refs() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo_with_options(&GitInitOptions {
        run_branchless_init: false,
        ..Default::default()
    })?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file("test2", 2)?;

    let show_refs_output = {
        let (stdout, _stderr) = git.run(&["show-ref"])?;
        insta::assert_snapshot!(stdout, @"62fc20d2a290daea0d52bdc2ed2ad4be6491010e refs/heads/master
");
        stdout
    };

    git.run(&["branchless", "move", "-d", &test1_oid.to_string()])?;

    {
        let (stdout, _stderr) = git.run(&["show-ref"])?;
        insta::assert_snapshot!(stdout, @r###"
        62fc20d2a290daea0d52bdc2ed2ad4be6491010e refs/heads/master
        "###);
        assert!(stdout == show_refs_output);
    }

    Ok(())
}
