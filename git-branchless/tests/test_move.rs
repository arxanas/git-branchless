use git_branchless_testing::{
    extract_hint_command, make_git, make_git_with_remote_repo, remove_rebase_lines, GitInitOptions,
    GitRunOptions, GitWrapperWithRemoteRepo,
};

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
        git.branchless(
            "move",
            &[
                "--on-disk",
                "-s",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d create test1.txt
        |\
        | o 4838e49 create test3.txt
        | |
        | @ a248207 create test4.txt
        |
        O 96d1c37 (master) create test2.txt
        "###);
    }

    // --in-memory
    {
        let (stdout, _stderr) = git.branchless(
            "move",
            &[
                "--in-memory",
                "--debug-dump-rebase-plan",
                "-s",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                commands: [
                    Reset {
                        target: Oid(
                            NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                        ),
                    },
                    Pick {
                        original_commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                        commit_to_apply_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                    },
                    Pick {
                        original_commit_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                        commit_to_apply_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                    },
                    RegisterExtraPostRewriteHook,
                ],
            },
        )
        Attempting rebase in-memory...
        [1/2] Committed as: 4838e49 create test3.txt
        [2/2] Committed as: a248207 create test4.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout a248207402822b7396cabe0f1011d8a7ce7daf1b
        :
        O 62fc20d create test1.txt
        |\
        | o 4838e49 create test3.txt
        | |
        | @ a248207 create test4.txt
        |
        O 96d1c37 (master) create test2.txt
        In-memory rebase succeeded.
        "###);

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d create test1.txt
        |\
        | o 4838e49 create test3.txt
        | |
        | @ a248207 create test4.txt
        |
        O 96d1c37 (master) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_insert_stick() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;

    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;

    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |
    @ 355e173 create test4.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--insert",
                "-s",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 4838e49 create test3.txt
        |
        @ a248207 create test4.txt
        |
        o 5a436ed create test2.txt
        "###);
    }

    // --in-memory
    {
        let (stdout, _stderr) = git.branchless(
            "move",
            &[
                "--in-memory",
                "--insert",
                // "--debug-dump-rebase-constraints",
                "--debug-dump-rebase-plan",
                "-s",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                commands: [
                    Reset {
                        target: Oid(
                            NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                        ),
                    },
                    Pick {
                        original_commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                        commit_to_apply_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                    },
                    Pick {
                        original_commit_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                        commit_to_apply_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                    },
                    Pick {
                        original_commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                        commit_to_apply_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                    },
                    RegisterExtraPostRewriteHook,
                ],
            },
        )
        Attempting rebase in-memory...
        [1/3] Committed as: 4838e49 create test3.txt
        [2/3] Committed as: a248207 create test4.txt
        [3/3] Committed as: 5a436ed create test2.txt
        branchless: processing 3 rewritten commits
        branchless: running command: <git-executable> checkout a248207402822b7396cabe0f1011d8a7ce7daf1b
        :
        O 62fc20d (master) create test1.txt
        |
        o 4838e49 create test3.txt
        |
        @ a248207 create test4.txt
        |
        o 5a436ed create test2.txt
        In-memory rebase succeeded.
        "###);

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 4838e49 create test3.txt
        |
        @ a248207 create test4.txt
        |
        o 5a436ed create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_single_stick() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |
    @ 355e173 create test4.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--exact",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o 96d1c37 create test2.txt
        | |
        | @ f57e36f create test4.txt
        |
        o 4838e49 create test3.txt
        "###);
    }

    // --in-memory
    {
        git.branchless(
            "move",
            &[
                "--in-memory",
                "--exact",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o 96d1c37 create test2.txt
        | |
        | @ f57e36f create test4.txt
        |
        o 4838e49 create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_range_stick() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;
    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |
    o 355e173 create test4.txt
    |
    @ f81d55c create test5.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--exact",
                &format!("{test2_oid}:{test3_oid}"),
                "-d",
                &test4_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o bf0d52a create test4.txt
        |\
        | o 44352d0 create test2.txt
        | |
        | o cf5eb24 create test3.txt
        |
        @ 848121c create test5.txt
        "###);
    }

    // --in-memory
    {
        git.branchless(
            "move",
            &[
                "--in-memory",
                "--exact",
                &format!("{test2_oid}:{test3_oid}"),
                "-d",
                &test4_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o bf0d52a create test4.txt
        |\
        | o 44352d0 create test2.txt
        | |
        | o cf5eb24 create test3.txt
        |
        @ 848121c create test5.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_noncontiguous_commits_stick() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;
    let test5_oid = git.commit_file("test5", 5)?;
    git.commit_file("test6", 6)?;
    let test7_oid = git.commit_file("test7", 7)?;
    git.commit_file("test8", 8)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |
    o 355e173 create test4.txt
    |
    o f81d55c create test5.txt
    |
    o 2831fb5 create test6.txt
    |
    o c8933b3 create test7.txt
    |
    @ 1edbaa1 create test8.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--exact",
                &format!("{test3_oid} + {test5_oid} + {test7_oid}"),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o 96d1c37 create test2.txt
        | |
        | o f57e36f create test4.txt
        | |
        | o d0c8705 create test6.txt
        | |
        | @ c458f41 create test8.txt
        |
        o 4838e49 create test3.txt
        |
        o b1f9efa create test5.txt
        |
        o 8577a96 create test7.txt
        "###);
    }

    // --in-memory
    {
        git.branchless(
            "move",
            &[
                "--in-memory",
                "--exact",
                &format!("{test3_oid} + {test5_oid} + {test7_oid}"),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o 96d1c37 create test2.txt
        | |
        | o f57e36f create test4.txt
        | |
        | o d0c8705 create test6.txt
        | |
        | @ c458f41 create test8.txt
        |
        o 4838e49 create test3.txt
        |
        o b1f9efa create test5.txt
        |
        o 8577a96 create test7.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_noncontiguous_ranges_stick() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;
    let test6_oid = git.commit_file("test6", 6)?;
    let test7_oid = git.commit_file("test7", 7)?;
    git.commit_file("test8", 8)?;
    let test9_oid = git.commit_file("test9", 9)?;
    let test10_oid = git.commit_file("test10", 10)?;
    git.commit_file("test11", 11)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |
    o 355e173 create test4.txt
    |
    o f81d55c create test5.txt
    |
    o 2831fb5 create test6.txt
    |
    o c8933b3 create test7.txt
    |
    o 1edbaa1 create test8.txt
    |
    o 384010f create test9.txt
    |
    o 52ebfa0 create test10.txt
    |
    @ b22a15b create test11.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--exact",
                &format!(
                    "{test3_oid}:{test4_oid} + {test6_oid}:{test7_oid} + {test9_oid}:{test10_oid}"
                ),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o 96d1c37 create test2.txt
        | |
        | o d2e18e3 create test5.txt
        | |
        | o a50e00a create test8.txt
        | |
        | @ dceb23f create test11.txt
        |
        o 4838e49 create test3.txt
        |
        o a248207 create test4.txt
        |
        o 133f783 create test6.txt
        |
        o c603422 create test7.txt
        |
        o 9c7387c create test9.txt
        |
        o fed2ec4 create test10.txt
        "###);
    }

    // --in-memory
    {
        git.branchless(
            "move",
            &[
                "--in-memory",
                "--exact",
                &format!(
                    "{test3_oid}:{test4_oid} + {test6_oid}:{test7_oid} + {test9_oid}:{test10_oid}"
                ),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o 96d1c37 create test2.txt
        | |
        | o d2e18e3 create test5.txt
        | |
        | o a50e00a create test8.txt
        | |
        | @ dceb23f create test11.txt
        |
        o 4838e49 create test3.txt
        |
        o a248207 create test4.txt
        |
        o 133f783 create test6.txt
        |
        o c603422 create test7.txt
        |
        o 9c7387c create test9.txt
        |
        o fed2ec4 create test10.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_contiguous_and_noncontiguous_stick() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;
    let test5_oid = git.commit_file("test5", 5)?;
    let test6_oid = git.commit_file("test6", 6)?;
    git.commit_file("test7", 7)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |
    o 355e173 create test4.txt
    |
    o f81d55c create test5.txt
    |
    o 2831fb5 create test6.txt
    |
    @ c8933b3 create test7.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--exact",
                &format!("{test3_oid} + {test5_oid}:{test6_oid}"),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o 96d1c37 create test2.txt
        | |
        | o f57e36f create test4.txt
        | |
        | @ c538d3e create test7.txt
        |
        o 4838e49 create test3.txt
        |
        o b1f9efa create test5.txt
        |
        o 500c9b3 create test6.txt
        "###);
    }

    // --in-memory
    {
        git.branchless(
            "move",
            &[
                "--in-memory",
                "--exact",
                &format!("{test3_oid} + {test5_oid}:{test6_oid}"),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o 96d1c37 create test2.txt
        | |
        | o f57e36f create test4.txt
        | |
        | @ c538d3e create test7.txt
        |
        o 4838e49 create test3.txt
        |
        o b1f9efa create test5.txt
        |
        o 500c9b3 create test6.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_insert_stick() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |
    @ 355e173 create test4.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--insert",
                "--exact",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 4838e49 create test3.txt
        |
        o d742fb9 create test2.txt
        |
        @ 8fcf7dd create test4.txt
        "###);
    }

    // --in-memory
    {
        git.branchless(
            "move",
            &[
                "--in-memory",
                "--insert",
                "--exact",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 4838e49 create test3.txt
        |
        o d742fb9 create test2.txt
        |
        @ 8fcf7dd create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_insert_swap() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;

    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |
    @ 355e173 create test4.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--insert",
                "--exact",
                &test2_oid.to_string(),
                "-d",
                &test3_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 4838e49 create test3.txt
        |
        o d742fb9 create test2.txt
        |
        @ 8fcf7dd create test4.txt
        "###);
    }

    // --in-memory
    {
        git.branchless(
            "move",
            &[
                "--in-memory",
                "--insert",
                "--exact",
                &test2_oid.to_string(),
                "-d",
                &test3_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 4838e49 create test3.txt
        |
        o d742fb9 create test2.txt
        |
        @ 8fcf7dd create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_insert_with_siblings() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file("test4", 4)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |\
    | o 70deb1e create test3.txt
    |
    @ f57e36f create test4.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--insert",
                "--exact",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 4838e49 create test3.txt
        |
        o d742fb9 create test2.txt
        |
        @ 8fcf7dd create test4.txt
        "###);
    }

    // --in-memory
    {
        git.branchless(
            "move",
            &[
                "--in-memory",
                "--insert",
                "--exact",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 4838e49 create test3.txt
        |
        o d742fb9 create test2.txt
        |
        @ 8fcf7dd create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_insert_range_stick() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |
    o 355e173 create test4.txt
    |
    @ f81d55c create test5.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--insert",
                "--exact",
                &format!("{test2_oid}:{test3_oid}"),
                "-d",
                &test4_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o bf0d52a create test4.txt
        |
        o 44352d0 create test2.txt
        |
        o cf5eb24 create test3.txt
        |
        @ 4acfdad create test5.txt
        "###);
    }

    // --in-memory
    {
        git.branchless(
            "move",
            &[
                "--in-memory",
                "--insert",
                "--exact",
                &format!("{test2_oid}:{test3_oid}"),
                "-d",
                &test4_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o bf0d52a create test4.txt
        |
        o 44352d0 create test2.txt
        |
        o cf5eb24 create test3.txt
        |
        @ 4acfdad create test5.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_insert_exact_noncontiguous_ranges_stick() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;
    let test6_oid = git.commit_file("test6", 6)?;
    let test7_oid = git.commit_file("test7", 7)?;
    git.commit_file("test8", 8)?;
    let test9_oid = git.commit_file("test9", 9)?;
    let test10_oid = git.commit_file("test10", 10)?;
    git.commit_file("test11", 11)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |
    o 355e173 create test4.txt
    |
    o f81d55c create test5.txt
    |
    o 2831fb5 create test6.txt
    |
    o c8933b3 create test7.txt
    |
    o 1edbaa1 create test8.txt
    |
    o 384010f create test9.txt
    |
    o 52ebfa0 create test10.txt
    |
    @ b22a15b create test11.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--insert",
                "--exact",
                &format!(
                    "{test3_oid}:{test4_oid} + {test6_oid}:{test7_oid} + {test9_oid}:{test10_oid}"
                ),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 4838e49 create test3.txt
        |
        o a248207 create test4.txt
        |
        o 133f783 create test6.txt
        |
        o c603422 create test7.txt
        |
        o 9c7387c create test9.txt
        |
        o fed2ec4 create test10.txt
        |
        o 324a4c2 create test2.txt
        |
        o e7c49ca create test5.txt
        |
        o acad4bd create test8.txt
        |
        @ 85ceeac create test11.txt
        "###);
    }

    // --in-memory
    {
        git.branchless(
            "move",
            &[
                "--in-memory",
                "--insert",
                "--exact",
                &format!(
                    "{test3_oid}:{test4_oid} + {test6_oid}:{test7_oid} + {test9_oid}:{test10_oid}"
                ),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 4838e49 create test3.txt
        |
        o a248207 create test4.txt
        |
        o 133f783 create test6.txt
        |
        o c603422 create test7.txt
        |
        o 9c7387c create test9.txt
        |
        o fed2ec4 create test10.txt
        |
        o 324a4c2 create test2.txt
        |
        o e7c49ca create test5.txt
        |
        o acad4bd create test8.txt
        |
        @ 85ceeac create test11.txt
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
        git.branchless(
            "move",
            &[
                "--on-disk",
                "-s",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;
        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d create test1.txt
            |\
            | o 4838e49 create test3.txt
            | |\
            | | o a248207 create test4.txt
            | |
            | @ b1f9efa create test5.txt
            |
            O 96d1c37 (master) create test2.txt
            "###);
        }
    }

    // in-memory
    {
        git.branchless(
            "move",
            &["-s", &test3_oid.to_string(), "-d", &test1_oid.to_string()],
        )?;
        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d create test1.txt
            |\
            | o 4838e49 create test3.txt
            | |\
            | | o a248207 create test4.txt
            | |
            | @ b1f9efa create test5.txt
            |
            O 96d1c37 (master) create test2.txt
            "###);
        }
    }

    Ok(())
}

#[test]
fn test_move_insert_in_place() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file("test3", 3)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |\
    | o 96d1c37 create test2.txt
    |
    @ 4838e49 create test3.txt
    "###);

    git.branchless(
        "move",
        &[
            "--insert",
            "-s",
            &test2_oid.to_string(),
            "-d",
            &test1_oid.to_string(),
        ],
    )?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        @ 70deb1e create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_insert_tree() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;

    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;

    git.run(&["checkout", &test1_oid.to_string()])?;
    git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |\
    | o 96d1c37 create test2.txt
    |
    o 4838e49 create test3.txt
    |
    @ a248207 create test4.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--insert",
                "-s",
                &test4_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;
        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d (master) create test1.txt
            |
            @ bf0d52a create test4.txt
            |\
            | o 44352d0 create test2.txt
            |
            o 0a4a701 create test3.txt
            "###);
        }
    }

    // in-memory
    {
        git.branchless(
            "move",
            &[
                "--insert",
                "-s",
                &test4_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;
        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d (master) create test1.txt
            |
            @ bf0d52a create test4.txt
            |\
            | o 44352d0 create test2.txt
            |
            o 0a4a701 create test3.txt
            "###);
        }
    }

    Ok(())
}

#[test]
fn test_move_exact_range_tree() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.run(&["checkout", &test3_oid.to_string()])?;
    git.commit_file("test5", 5)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |\
    | o 355e173 create test4.txt
    |
    @ 9ea1b36 create test5.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--exact",
                &format!("{test2_oid}:{test3_oid}"),
                "-d",
                &test4_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o bf0d52a create test4.txt
        | |
        | o 44352d0 create test2.txt
        | |
        | o cf5eb24 create test3.txt
        |
        @ ea7aa06 create test5.txt
        "###);
    }

    // in-memory
    {
        git.branchless(
            "move",
            &[
                "--exact",
                &format!("{test2_oid}:{test3_oid}"),
                "-d",
                &test4_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o bf0d52a create test4.txt
        | |
        | o 44352d0 create test2.txt
        | |
        | o cf5eb24 create test3.txt
        |
        @ ea7aa06 create test5.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_range_with_leaves() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;
    git.run(&["checkout", &test3_oid.to_string()])?;
    let test5_oid = git.commit_file("test5", 5)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |\
    | o 355e173 create test4.txt
    |
    @ 9ea1b36 create test5.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--exact",
                &format!("{test3_oid}+{test5_oid}"),
                "-d",
                &test2_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | o 70deb1e create test3.txt
        | |
        | @ 9ea1b36 create test5.txt
        |
        o f57e36f create test4.txt
        "###);
    }

    // in-memory
    {
        git.branchless(
            "move",
            &[
                "--exact",
                &format!("{test3_oid}+{test5_oid}"),
                "-d",
                &test2_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | o 70deb1e create test3.txt
        | |
        | @ 9ea1b36 create test5.txt
        |
        o f57e36f create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_range_just_leaves() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.run(&["checkout", "HEAD^"])?;
    let test5_oid = git.commit_file("test5", 5)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |\
    | o 355e173 create test4.txt
    |
    @ 9ea1b36 create test5.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--exact",
                &format!("{test4_oid}+{test5_oid}"),
                "-d",
                &test2_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | o 70deb1e create test3.txt
        |\
        | o f57e36f create test4.txt
        |
        @ d2e18e3 create test5.txt
        "###);
    }

    // in-memory
    {
        git.branchless(
            "move",
            &[
                "--exact",
                &format!("{test4_oid}+{test5_oid}"),
                "-d",
                &test2_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | o 70deb1e create test3.txt
        |\
        | o f57e36f create test4.txt
        |
        @ d2e18e3 create test5.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_range_with_multiple_heads() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;
    git.run(&["checkout", "HEAD^^"])?;
    let test6_oid = git.commit_file("test6", 6)?;
    git.commit_file("test7", 7)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |\
    | o 355e173 create test4.txt
    | |
    | o f81d55c create test5.txt
    |
    o eaf39e9 create test6.txt
    |
    @ 0094d46 create test7.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--exact",
                &format!("{test3_oid}+{test4_oid}+{test6_oid}"),
                "-d",
                &test2_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | o 70deb1e create test3.txt
        | |\
        | | o 355e173 create test4.txt
        | |
        | o eaf39e9 create test6.txt
        |\
        | o d2e18e3 create test5.txt
        |
        @ 66602bc create test7.txt
        "###);
    }

    // in-memory
    {
        git.branchless(
            "move",
            &[
                "--exact",
                &format!("{test3_oid}+{test4_oid}+{test6_oid}"),
                "-d",
                &test2_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | o 70deb1e create test3.txt
        | |\
        | | o 355e173 create test4.txt
        | |
        | o eaf39e9 create test6.txt
        |\
        | o d2e18e3 create test5.txt
        |
        @ 66602bc create test7.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_range_with_leaves_and_descendent_components() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;
    let test5_oid = git.commit_file("test5", 5)?;
    git.run(&["checkout", &test3_oid.to_string()])?;
    let test6_oid = git.commit_file("test6", 6)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 70deb1e create test3.txt
    |\
    | o 355e173 create test4.txt
    | |
    | o f81d55c create test5.txt
    |
    @ eaf39e9 create test6.txt
    "###);

    // --on-disk
    {
        let git = git.duplicate_repo()?;
        git.branchless(
            "move",
            &[
                "--on-disk",
                "--exact",
                &format!("{test3_oid}+{test5_oid}+{test6_oid}"),
                "-d",
                &test2_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | o 70deb1e create test3.txt
        | |\
        | | o 9ea1b36 create test5.txt
        | |
        | @ eaf39e9 create test6.txt
        |
        o f57e36f create test4.txt
        "###);
    }

    // in-memory
    {
        git.branchless(
            "move",
            &[
                "--exact",
                &format!("{test3_oid}+{test5_oid}+{test6_oid}"),
                "-d",
                &test2_oid.to_string(),
            ],
        )?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | o 70deb1e create test3.txt
        | |\
        | | o 9ea1b36 create test5.txt
        | |
        | @ eaf39e9 create test6.txt
        |
        o f57e36f create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_exact_ranges_with_merge_commits_betwixt_not_supported() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.run(&["checkout", &test1_oid.to_string()])?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;
    git.run(&["merge", &test3_oid.to_string()])?;
    let test6_oid = git.commit_file("test6", 6)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |\
    | o 96d1c37 create test2.txt
    | |
    | o 70deb1e create test3.txt
    | & (merge) 01a3b9b Merge commit '70deb1e28791d8e7dd5a1f0c871a51b91282562f' into HEAD
    |
    o bf0d52a create test4.txt
    |
    o 848121c create test5.txt
    |
    | & (merge) 70deb1e create test3.txt
    |/
    o 01a3b9b Merge commit '70deb1e28791d8e7dd5a1f0c871a51b91282562f' into HEAD
    |
    @ 8fb4e4a create test6.txt
    "###);

    let (stdout, stderr) = git.branchless_with_options(
        "move",
        &[
            "--exact",
            &format!("{test2_oid}+{test4_oid}+{test6_oid}"),
            "-d",
            &test2_oid.to_string(),
        ],
        &GitRunOptions {
            expected_exit_code: 1,
            ..Default::default()
        },
    )?;
    insta::assert_snapshot!(stderr, @"");
    insta::assert_snapshot!(stdout, @r###"
    This operation cannot be completed because the commit at 8fb4e4aded06dd3b97723794474832a928370f9a
    has multiple possible parents also being moved. Please retry this operation
    without this commit, or with only 1 possible parent.
    "###);

    Ok(())
}

#[test]
fn test_move_exact_range_one_side_of_merged_stack_without_base_and_merge_commits(
) -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.run(&["checkout", &test2_oid.to_string()])?;
    git.commit_file("test5", 5)?;
    git.commit_file("test6", 6)?;
    git.run(&["merge", &test4_oid.to_string()])?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |\
    | o 70deb1e create test3.txt
    | |
    | o 355e173 create test4.txt
    | & (merge) 178e00f Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    |
    o d2e18e3 create test5.txt
    |
    o d43fec8 create test6.txt
    |
    | & (merge) 355e173 create test4.txt
    |/
    @ 178e00f Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    "###);

    git.branchless(
        "move",
        &[
            "--exact",
            &format!("{test3_oid}+{test4_oid}"),
            "-d",
            &test1_oid.to_string(),
        ],
    )?;

    let stdout = git.smartlog()?;
    // FIXME: This output is correct except for a known issue involving moving
    // merge commits. (See `test_move_merge_commit_both_parents`.)
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |\
    | o 96d1c37 create test2.txt
    | |\
    | | x 70deb1e (rewritten as 4838e49b) create test3.txt
    | | |
    | | x 355e173 (rewritten as a2482074) create test4.txt
    | | & (merge) 178e00f Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    | |
    | o d2e18e3 create test5.txt
    | |
    | o d43fec8 create test6.txt
    | |
    | | & (merge) 355e173 (rewritten as a2482074) create test4.txt
    | |/
    | @ 178e00f Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    |
    o 4838e49 create test3.txt
    |
    o a248207 create test4.txt
    hint: there is 1 abandoned commit in your commit graph
    hint: to fix this, run: git restack
    hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
    "###);

    Ok(())
}

#[test]
fn test_move_exact_range_one_side_of_merged_stack_including_base_and_merge_commits(
) -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.run(&["checkout", &test2_oid.to_string()])?;
    git.commit_file("test5", 5)?;
    git.commit_file("test6", 6)?;
    git.run(&["merge", &test4_oid.to_string()])?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |\
    | o 70deb1e create test3.txt
    | |
    | o 355e173 create test4.txt
    | & (merge) 178e00f Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    |
    o d2e18e3 create test5.txt
    |
    o d43fec8 create test6.txt
    |
    | & (merge) 355e173 create test4.txt
    |/
    @ 178e00f Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    "###);

    git.branchless(
        "move",
        &[
            "--exact",
            &format!("{}+{}+{}+{}", test2_oid, test3_oid, test4_oid, "178e00f"),
            "-d",
            &test1_oid.to_string(),
        ],
    )?;

    let stdout = git.smartlog()?;
    // FIXME: This output is correct except for a known issue involving moving
    // merge commits. (See `test_move_merge_commit_both_parents`.)
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |\
    | o 96d1c37 create test2.txt
    | |\
    | | o 70deb1e create test3.txt
    | | |
    | | o 355e173 create test4.txt
    | | & (merge) 178e00f Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    | |
    | x d2e18e3 (rewritten as ea7aa064) create test5.txt
    | |
    | x d43fec8 (rewritten as da42aeb4) create test6.txt
    | |
    | | & (merge) 355e173 create test4.txt
    | |/
    | @ 178e00f Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    |
    o ea7aa06 create test5.txt
    |
    o da42aeb create test6.txt
    hint: there is 1 abandoned commit in your commit graph
    hint: to fix this, run: git restack
    hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
    "###);

    Ok(())
}

#[test]
fn test_move_exact_range_two_partial_components_of_merged_stack() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.run(&["checkout", &test2_oid.to_string()])?;
    let test5_oid = git.commit_file("test5", 5)?;
    git.commit_file("test6", 6)?;
    git.run(&["merge", &test4_oid.to_string()])?;

    // Given this graph: 1-2-3-4-7
    //                      \5-6/
    // Moving 2,3,6,7 (leaving 4,5) should produce:
    // 1-2-3
    //  | \6-7
    //  +4
    //  \5
    // FIXME Is it Ok that 3&7 are no longer directly connected?

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |
    o 96d1c37 create test2.txt
    |\
    | o 70deb1e create test3.txt
    | |
    | o 355e173 create test4.txt
    | & (merge) 178e00f Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    |
    o d2e18e3 create test5.txt
    |
    o d43fec8 create test6.txt
    |
    | & (merge) 355e173 create test4.txt
    |/
    @ 178e00f Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    "###);

    git.branchless(
        "move",
        &[
            "--exact",
            &format!("{test2_oid}:: - {test4_oid} - {test5_oid}"),
            "-d",
            &test1_oid.to_string(),
        ],
    )?;

    let stdout = git.smartlog()?;
    // FIXME: This output is correct except for a known issue involving moving
    // merge commits. (See `test_move_merge_commit_both_parents`.)
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |\
    | o 96d1c37 create test2.txt
    | |\
    | | o 70deb1e create test3.txt
    | | |
    | | x 355e173 (rewritten as bf0d52a6) create test4.txt
    | | & (merge) 178e00f Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    | |\
    | | x d2e18e3 (rewritten as ea7aa064) create test5.txt
    | | |
    | | x d43fec8 (rewritten as d071649c) create test6.txt
    | | |
    | | | & (merge) 355e173 (rewritten as bf0d52a6) create test4.txt
    | | |/
    | | @ 178e00f Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
    | |
    | o d071649 create test6.txt
    |\
    | o bf0d52a create test4.txt
    |
    o ea7aa06 create test5.txt
    hint: there is 1 abandoned commit in your commit graph
    hint: to fix this, run: git restack
    hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
    "###);

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

        git.branchless(
            "move",
            &[
                "--on-disk",
                "-f",
                "-s",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d create test1.txt
            |\
            : o 96d1c37 create test2.txt
            :
            @ a248207 (> master) create test4.txt
            "###);
        }
    }

    // --in-memory
    {
        {
            let (stdout, _stderr) = git.branchless(
                "move",
                &[
                    "--in-memory",
                    "--debug-dump-rebase-plan",
                    "-f",
                    "-s",
                    &test3_oid.to_string(),
                    "-d",
                    &test1_oid.to_string(),
                ],
            )?;
            insta::assert_snapshot!(stdout, @r###"
            Rebase plan: Some(
                RebasePlan {
                    first_dest_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                    commands: [
                        Reset {
                            target: Oid(
                                NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                            ),
                        },
                        Pick {
                            original_commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                            commit_to_apply_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                        },
                        DetectEmptyCommit {
                            commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                        },
                        Pick {
                            original_commit_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                            commit_to_apply_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                        },
                        DetectEmptyCommit {
                            commit_oid: NonZeroOid(355e173bf9c5d2efac2e451da0cdad3fb82b869a),
                        },
                        RegisterExtraPostRewriteHook,
                    ],
                },
            )
            Attempting rebase in-memory...
            [1/2] Committed as: 4838e49 create test3.txt
            [2/2] Committed as: a248207 create test4.txt
            branchless: processing 1 update: branch master
            branchless: processing 2 rewritten commits
            branchless: running command: <git-executable> checkout master
            :
            O 62fc20d create test1.txt
            |\
            : o 96d1c37 create test2.txt
            :
            @ a248207 (> master) create test4.txt
            In-memory rebase succeeded.
            "###);
        }

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d create test1.txt
            |\
            : o 96d1c37 create test2.txt
            :
            @ a248207 (> master) create test4.txt
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
        let (stdout, _stderr) = git.branchless_with_options(
            "move",
            &["--source", &other_oid.to_string()],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        This operation would cause a merge conflict:
        - (1 conflicting file) e85d25c create conflict.txt
        To resolve merge conflicts, retry this operation with the --merge option.
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless_with_options(
            "move",
            &[
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
                    Reset {
                        target: Oid(
                            NonZeroOid(202143f2fdfc785285ab097422f6a695ff1d93cb),
                        ),
                    },
                    Pick {
                        original_commit_oid: NonZeroOid(e85d25c772a05b5c73ea8ec43881c12bbf588848),
                        commit_to_apply_oid: NonZeroOid(e85d25c772a05b5c73ea8ec43881c12bbf588848),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(e85d25c772a05b5c73ea8ec43881c12bbf588848),
                    },
                    RegisterExtraPostRewriteHook,
                ],
            },
        )
        Attempting rebase in-memory...
        Failed to merge in-memory, trying again on-disk...
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
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        @ 202143f create conflict.txt
        |
        o 42951b5 create conflict.txt
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
        let (stdout, _stderr) = git.branchless(
            "move",
            &["--debug-dump-rebase-plan", "--base", &test3_oid.to_string()],
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: NonZeroOid(bf0d52a607f693201512a43b6b5a70b2a275e0ad),
                commands: [
                    Reset {
                        target: Oid(
                            NonZeroOid(bf0d52a607f693201512a43b6b5a70b2a275e0ad),
                        ),
                    },
                    Pick {
                        original_commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                        commit_to_apply_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                    },
                    Pick {
                        original_commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                        commit_to_apply_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(70deb1e28791d8e7dd5a1f0c871a51b91282562f),
                    },
                    RegisterExtraPostRewriteHook,
                ],
            },
        )
        Attempting rebase in-memory...
        [1/2] Committed as: 44352d0 create test2.txt
        [2/2] Committed as: cf5eb24 create test3.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout master
        :
        @ bf0d52a (> master) create test4.txt
        |
        o 44352d0 create test2.txt
        |
        o cf5eb24 create test3.txt
        In-memory rebase succeeded.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ bf0d52a (> master) create test4.txt
        |
        o 44352d0 create test2.txt
        |
        o cf5eb24 create test3.txt
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
        let (stdout, stderr) =
            git.branchless("move", &["-b", "HEAD", "-d", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: creating working copy snapshot
        Previous HEAD position was a248207 create test4.txt
        branchless: processing 1 update: ref HEAD
        HEAD is now at 355e173 create test4.txt
        branchless: processing checkout
        "###);
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/2] Committed as: 70deb1e create test3.txt
        [2/2] Committed as: 355e173 create test4.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout 355e173bf9c5d2efac2e451da0cdad3fb82b869a
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        o 70deb1e create test3.txt
        |
        @ 355e173 create test4.txt
        In-memory rebase succeeded.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        o 70deb1e create test3.txt
        |
        @ 355e173 create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_checkout_new_head() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.branchless("prev", &[])?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) =
            git.branchless("move", &["--debug-dump-rebase-plan", "-d", "master"])?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                commands: [
                    Reset {
                        target: Oid(
                            NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                        ),
                    },
                    Pick {
                        original_commit_oid: NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                        commit_to_apply_oid: NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                    },
                    RegisterExtraPostRewriteHook,
                ],
            },
        )
        Attempting rebase in-memory...
        [1/1] Committed as: 96d1c37 create test2.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout 96d1c37a3d4363611c49f7e52186e189a04c531f
        :
        O 62fc20d (master) create test1.txt
        |
        @ 96d1c37 create test2.txt
        In-memory rebase succeeded.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        @ 96d1c37 create test2.txt
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
        let (stdout, _stderr) = git.run(&["branch", "--show-current"])?;
        insta::assert_snapshot!(stdout, @"master");
    }

    {
        let (stdout, _stderr) = git.branchless(
            "move",
            &[
                "--debug-dump-rebase-plan",
                "-d",
                &test2_oid.to_string(),
                "-f",
            ],
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                commands: [
                    Reset {
                        target: Oid(
                            NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                        ),
                    },
                    Pick {
                        original_commit_oid: NonZeroOid(98b9119d16974f372e76cb64a3b77c528fc0b18b),
                        commit_to_apply_oid: NonZeroOid(98b9119d16974f372e76cb64a3b77c528fc0b18b),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(98b9119d16974f372e76cb64a3b77c528fc0b18b),
                    },
                    RegisterExtraPostRewriteHook,
                ],
            },
        )
        Attempting rebase in-memory...
        [1/1] Committed as: 70deb1e create test3.txt
        branchless: processing 1 update: branch master
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout master
        :
        @ 70deb1e (> master) create test3.txt
        In-memory rebase succeeded.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 70deb1e (> master) create test3.txt
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
        let (stdout, stderr) = git.branchless_with_options(
            "move",
            &["--debug-dump-rebase-plan", "-b", "HEAD^"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        This operation failed because it would introduce a cycle:
        ,-> 70deb1e create test3.txt
        |   96d1c37 create test2.txt
        `-- 70deb1e create test3.txt
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        @ 70deb1e create test3.txt
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

    git.write_file_txt("test2", "conflicting contents")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "conflicting test2"])?;

    {
        let (stdout, stderr) = git.branchless_with_options(
            "move",
            &["-d", "master", "--in-memory"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        This operation would cause a merge conflict:
        - (1 conflicting file) 081b474 conflicting test2
        To resolve merge conflicts, retry this operation with the --merge option.
        "###);
    }

    {
        let (stdout, stderr) = git.branchless_with_options(
            "move",
            &["-d", "master", "--in-memory", "--merge"],
            &GitRunOptions {
                expected_exit_code: 2,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @r###"
        error: The argument '--in-memory' cannot be used with '--merge'

        Usage: git-branchless move --dest <DEST> --in-memory

        For more information try '--help'
        "###);
        insta::assert_snapshot!(stdout, @"");
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
    git.branchless("move", &["-d", "master"])?;
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
        let (stdout, stderr) = git.branchless(
            "move",
            &[
                "--debug-dump-rebase-plan",
                "-s",
                "HEAD",
                "-d",
                "master",
                "--in-memory",
            ],
        )?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: creating working copy snapshot
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
                    Reset {
                        target: Oid(
                            NonZeroOid(f777ecc9b0db5ed372b2615695191a8a17f79f24),
                        ),
                    },
                    Pick {
                        original_commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                        commit_to_apply_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                    },
                    DetectEmptyCommit {
                        commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                    },
                    RegisterExtraPostRewriteHook,
                ],
            },
        )
        Attempting rebase in-memory...
        [1/1] Committed as: fe65c1f create test2.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout fe65c1fe15584744e649b2c79d4cf9b0d878f92e
        O f777ecc (master) create initial.txt
        |\
        | o 62fc20d create test1.txt
        |
        @ fe65c1f create test2.txt
        In-memory rebase succeeded.
        "###);
    }

    git.run(&["checkout", &test1_oid.to_string()])?;

    {
        let (stdout, stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | @ 62fc20d create test1.txt
        |
        o fe65c1f create test2.txt
        "###);
    }

    git.run(&["gc", "--prune=now"])?;

    {
        let (stdout, stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | @ 62fc20d create test1.txt
        |
        o fe65c1f create test2.txt
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
        let (stdout, _stderr) = git.branchless(
            "move",
            &[
                "-f",
                "-s",
                &test3_oid.to_string(),
                "-d",
                &test1_oid.to_string(),
            ],
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/3] Committed as: 4838e49 create test3.txt
        [2/3] Committed as: a248207 create test4.txt
        [3/3] Committed as: 566e434 create test5.txt
        branchless: processing 1 update: branch master
        branchless: processing 3 rewritten commits
        branchless: running command: <git-executable> checkout master
        :
        O 62fc20d create test1.txt
        |\
        : o 96d1c37 create test2.txt
        :
        @ 566e434 (> master) create test5.txt
        In-memory rebase succeeded.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d create test1.txt
        |\
        : o 96d1c37 create test2.txt
        :
        @ 566e434 (> master) create test5.txt
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
            let (stdout, _stderr) = git.run(&["branch", "--show-current"])?;
            insta::assert_snapshot!(stdout, @"");
        }

        {
            let (stdout, stderr) = git.branchless(
                "move",
                &["--on-disk", "-s", "foo", "-d", &test1_oid.to_string()],
            )?;
            insta::assert_snapshot!(stderr, @r###"
            branchless: processing 1 update: ref HEAD
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: 4838e49 create test3.txt
            Executing: git branchless hook-detect-empty-commit 70deb1e28791d8e7dd5a1f0c871a51b91282562f
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: a248207 create test4.txt
            Executing: git branchless hook-detect-empty-commit 355e173bf9c5d2efac2e451da0cdad3fb82b869a
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: 566e434 create test5.txt
            Executing: git branchless hook-detect-empty-commit f81d55c0d520ff8d02ef9294d95156dcb78a5255
            Executing: git branchless hook-register-extra-post-rewrite-hook
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
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d create test1.txt
            |\
            | o 4838e49 (foo) create test3.txt
            | |
            | o a248207 create test4.txt
            | |
            | @ 566e434 (bar) create test5.txt
            |
            O 96d1c37 (master) create test2.txt
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["branch", "--show-current"])?;
            insta::assert_snapshot!(stdout, @"");
        }

        {
            // There should be no branches left to restack.
            let (stdout, _stderr) = git.branchless("restack", &[])?;
            insta::assert_snapshot!(stdout, @r###"
            No abandoned commits to restack.
            No abandoned branches to restack.
            :
            O 62fc20d create test1.txt
            |\
            | o 4838e49 (foo) create test3.txt
            | |
            | o a248207 create test4.txt
            | |
            | @ 566e434 (bar) create test5.txt
            |
            O 96d1c37 (master) create test2.txt
            "###);
        }
    }

    {
        {
            let (stdout, stderr) = git.branchless(
                "move",
                &["--in-memory", "-s", "foo", "-d", &test1_oid.to_string()],
            )?;
            insta::assert_snapshot!(stderr, @r###"
            branchless: creating working copy snapshot
            Previous HEAD position was f81d55c create test5.txt
            Switched to branch 'bar'
            branchless: processing checkout
            "###);
            insta::assert_snapshot!(stdout, @r###"
            Attempting rebase in-memory...
            [1/3] Committed as: 4838e49 create test3.txt
            [2/3] Committed as: a248207 create test4.txt
            [3/3] Committed as: 566e434 create test5.txt
            branchless: processing 2 updates: branch bar, branch foo
            branchless: processing 3 rewritten commits
            branchless: running command: <git-executable> checkout bar
            :
            O 62fc20d create test1.txt
            |\
            | o 4838e49 (foo) create test3.txt
            | |
            | o a248207 create test4.txt
            | |
            | @ 566e434 (> bar) create test5.txt
            |
            O 96d1c37 (master) create test2.txt
            In-memory rebase succeeded.
            "###);
        }

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 62fc20d create test1.txt
            |\
            | o 4838e49 (foo) create test3.txt
            | |
            | o a248207 create test4.txt
            | |
            | @ 566e434 (> bar) create test5.txt
            |
            O 96d1c37 (master) create test2.txt
            "###);
        }

        {
            // There should be no branches left to restack.
            let (stdout, _stderr) = git.branchless("restack", &[])?;
            insta::assert_snapshot!(stdout, @r###"
            No abandoned commits to restack.
            No abandoned branches to restack.
            :
            O 62fc20d create test1.txt
            |\
            | o 4838e49 (foo) create test3.txt
            | |
            | o a248207 create test4.txt
            | |
            | @ 566e434 (> bar) create test5.txt
            |
            O 96d1c37 (master) create test2.txt
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
            let (stdout, stderr) =
                git.branchless("move", &["--on-disk", "-b", "HEAD", "-d", "master"])?;
            insta::assert_snapshot!(stderr, @r###"
            branchless: processing 1 update: ref HEAD
            Executing: git branchless hook-skip-upstream-applied-commit 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: fa46633 create test2.txt
            Executing: git branchless hook-detect-empty-commit 96d1c37a3d4363611c49f7e52186e189a04c531f
            Executing: git branchless hook-register-extra-post-rewrite-hook
            branchless: processing 2 rewritten commits
            branchless: processing 1 update: branch should-be-deleted
            Successfully rebased and updated detached HEAD.
            "###);
            insta::assert_snapshot!(stdout, @r###"
            branchless: running command: <git-executable> diff --quiet
            Calling Git for on-disk rebase...
            branchless: running command: <git-executable> rebase --continue
            Skipping commit (was already applied upstream): 62fc20d create test1.txt
            "###);
        }

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 047b7ad (master) create test1.txt
            |
            @ fa46633 create test2.txt
            "###);
        }
    }

    // --in-memory
    {
        {
            let (stdout, stderr) =
                git.branchless("move", &["--in-memory", "-b", "HEAD", "-d", "master"])?;
            insta::assert_snapshot!(stderr, @r###"
            branchless: creating working copy snapshot
            Previous HEAD position was 96d1c37 create test2.txt
            branchless: processing 1 update: ref HEAD
            HEAD is now at fa46633 create test2.txt
            branchless: processing checkout
            "###);
            insta::assert_snapshot!(stdout, @r###"
            Attempting rebase in-memory...
            [1/2] Skipped commit (was already applied upstream): 62fc20d create test1.txt
            [2/2] Committed as: fa46633 create test2.txt
            branchless: processing 1 update: branch should-be-deleted
            branchless: processing 2 rewritten commits
            branchless: running command: <git-executable> checkout fa46633239bfa767036e41a77b67258286e4ddb9
            :
            O 047b7ad (master) create test1.txt
            |
            @ fa46633 create test2.txt
            In-memory rebase succeeded.
            "###);
        }

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 047b7ad (master) create test1.txt
            |
            @ fa46633 create test2.txt
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
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            O f777ecc create initial.txt
            |\
            | o 62fc20d create test1.txt
            | |
            | o 96d1c37 create test2.txt
            |
            @ de4a1fe (> master) squashed test1 and test2
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["branch", "--show-current"])?;
            insta::assert_snapshot!(stdout, @"master");
        }

        {
            let (stdout, stderr) = git.branchless(
                "move",
                &["--on-disk", "-b", &test2_oid.to_string(), "-d", "master"],
            )?;
            insta::assert_snapshot!(stderr, @r###"
            branchless: processing 1 update: ref HEAD
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: e7bcdd6 create test1.txt
            Executing: git branchless hook-detect-empty-commit 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: 12d361a create test2.txt
            Executing: git branchless hook-detect-empty-commit 96d1c37a3d4363611c49f7e52186e189a04c531f
            Executing: git branchless hook-register-extra-post-rewrite-hook
            branchless: processing 4 rewritten commits
            branchless: creating working copy snapshot
            branchless: running command: <git-executable> checkout master
            Switched to branch 'master'
            branchless: processing checkout
            :
            @ de4a1fe (> master) squashed test1 and test2
            Successfully rebased and updated detached HEAD.
            "###);
            insta::assert_snapshot!(stdout, @r###"
            hint: you can omit the --dest flag in this case, as it defaults to HEAD
            hint: disable this hint by running: git config --global branchless.hint.moveImplicitHeadArgument false
            branchless: running command: <git-executable> diff --quiet
            Calling Git for on-disk rebase...
            branchless: running command: <git-executable> rebase --continue
            Skipped now-empty commit: e7bcdd6 create test1.txt
            Skipped now-empty commit: 12d361a create test2.txt
            "###);
        }

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            @ de4a1fe (> master) squashed test1 and test2
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["branch", "--show-current"])?;
            insta::assert_snapshot!(stdout, @"master");
        }
    }

    // --in-memory
    {
        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            O f777ecc create initial.txt
            |\
            | o 62fc20d create test1.txt
            | |
            | o 96d1c37 create test2.txt
            |
            @ de4a1fe (> master) squashed test1 and test2
            "###);
        }

        {
            let (stdout, stderr) = git.branchless(
                "move",
                &["--in-memory", "-b", &test2_oid.to_string(), "-d", "master"],
            )?;
            insta::assert_snapshot!(stderr, @r###"
            branchless: creating working copy snapshot
            Switched to branch 'master'
            branchless: processing checkout
            "###);
            insta::assert_snapshot!(stdout, @r###"
            hint: you can omit the --dest flag in this case, as it defaults to HEAD
            hint: disable this hint by running: git config --global branchless.hint.moveImplicitHeadArgument false
            Attempting rebase in-memory...
            [1/2] Skipped now-empty commit: e7bcdd6 create test1.txt
            [2/2] Skipped now-empty commit: 12d361a create test2.txt
            branchless: processing 2 rewritten commits
            branchless: running command: <git-executable> checkout master
            :
            @ de4a1fe (> master) squashed test1 and test2
            In-memory rebase succeeded.
            "###);
        }

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            @ de4a1fe (> master) squashed test1 and test2
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
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
        : o 62fc20d create test1.txt
        : |
        : o 96d1c37 (work) create test2.txt
        : |
        : o ffcba55 (more-work) create test3.txt
        :
        @ 91c5ce6 (> master) create test2.txt
        "###);
    }

    // --on-disk
    {
        let git = git.duplicate_repo()?;

        {
            git.run(&["checkout", "work"])?;
            let (stdout, stderr) =
                git.branchless("move", &["--on-disk", "-b", "HEAD", "-d", "master"])?;
            insta::assert_snapshot!(stderr, @r###"
            branchless: processing 1 update: ref HEAD
            Executing: git branchless hook-skip-upstream-applied-commit 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
            Executing: git branchless hook-skip-upstream-applied-commit 96d1c37a3d4363611c49f7e52186e189a04c531f
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: 012efd6 create test3.txt
            Executing: git branchless hook-detect-empty-commit ffcba554683d83de283de084a7d3896e332bbcdb
            Executing: git branchless hook-register-extra-post-rewrite-hook
            branchless: processing 3 rewritten commits
            branchless: processing 2 updates: branch more-work, branch work
            branchless: creating working copy snapshot
            branchless: running command: <git-executable> checkout master
            Previous HEAD position was 012efd6 create test3.txt
            Switched to branch 'master'
            branchless: processing checkout
            :
            @ 91c5ce6 (> master) create test2.txt
            |
            o 012efd6 (more-work) create test3.txt
            Successfully rebased and updated detached HEAD.
            "###);
            insta::assert_snapshot!(stdout, @r###"
            branchless: running command: <git-executable> diff --quiet
            Calling Git for on-disk rebase...
            branchless: running command: <git-executable> rebase --continue
            Skipping commit (was already applied upstream): 62fc20d create test1.txt
            Skipping commit (was already applied upstream): 96d1c37 create test2.txt
            "###);
        }

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            @ 91c5ce6 (> master) create test2.txt
            |
            o 012efd6 (more-work) create test3.txt
            "###);
        }
    }
    // --in-memory
    {
        {
            git.run(&["checkout", "work"])?;
            let (stdout, stderr) =
                git.branchless("move", &["--in-memory", "-b", "HEAD", "-d", "master"])?;
            insta::assert_snapshot!(stderr, @r###"
            branchless: creating working copy snapshot
            Previous HEAD position was 96d1c37 create test2.txt
            Switched to branch 'master'
            branchless: processing checkout
            "###);
            insta::assert_snapshot!(stdout, @r###"
            Attempting rebase in-memory...
            [1/3] Skipped commit (was already applied upstream): 62fc20d create test1.txt
            [2/3] Skipped commit (was already applied upstream): 96d1c37 create test2.txt
            [3/3] Committed as: 012efd6 create test3.txt
            branchless: processing 2 updates: branch more-work, branch work
            branchless: processing 3 rewritten commits
            branchless: running command: <git-executable> checkout master
            :
            @ 91c5ce6 (> master) create test2.txt
            |
            o 012efd6 (more-work) create test3.txt
            In-memory rebase succeeded.
            "###);
        }

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            @ 91c5ce6 (> master) create test2.txt
            |
            o 012efd6 (more-work) create test3.txt
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
        git.write_file_txt("test3", "new contents")?;
        let (stdout, stderr) = git.branchless_with_options(
            "move",
            &["--on-disk", "-d", "master"],
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
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
        | o fe65c1f create test2.txt
        | & (merge) 28790c7 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
        |\
        | @ 98b9119 create test3.txt
        | |
        | | & (merge) fe65c1f create test2.txt
        | |/
        | o 28790c7 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
        |
        O 62fc20d (master) create test1.txt
        "###);
    }

    // --on-disk
    {
        let git = git.duplicate_repo()?;

        {
            let (stdout, stderr) = git.branchless(
                "move",
                &[
                    "--debug-dump-rebase-constraints",
                    "--debug-dump-rebase-plan",
                    "--merge",
                    "--on-disk",
                    "-s",
                    &test2_oid.to_string(),
                    "-d",
                    "master",
                ],
            )?;
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
                        Reset {
                            target: Oid(
                                NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
                            ),
                        },
                        Pick {
                            original_commit_oid: NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                            commit_to_apply_oid: NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                        },
                        DetectEmptyCommit {
                            commit_oid: NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
                        },
                        CreateLabel {
                            label_name: "parent-3",
                        },
                        Reset {
                            target: Oid(
                                NonZeroOid(98b9119d16974f372e76cb64a3b77c528fc0b18b),
                            ),
                        },
                        Merge {
                            commit_oid: NonZeroOid(28790c73f13f38ce0d3beb6cfeb2d818b32bcd09),
                            commits_to_merge: [
                                Label(
                                    "parent-3",
                                ),
                            ],
                        },
                        RegisterExtraPostRewriteHook,
                    ],
                },
            )
            branchless: running command: <git-executable> diff --quiet
            Calling Git for on-disk rebase...
            branchless: running command: <git-executable> rebase --continue
            "###);
            insta::assert_snapshot!(stderr, @r###"
            branchless: processing 1 update: ref HEAD
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: 96d1c37 create test2.txt
            Executing: git branchless hook-detect-empty-commit fe65c1fe15584744e649b2c79d4cf9b0d878f92e
            branchless: processing 1 update: ref refs/rewritten/parent-3
            branchless: processing 1 update: ref HEAD
            Executing: git branchless hook-register-extra-post-rewrite-hook
            branchless: processing 2 rewritten commits
            branchless: creating working copy snapshot
            branchless: running command: <git-executable> checkout 98b9119d16974f372e76cb64a3b77c528fc0b18b
            Previous HEAD position was 5a6a761 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
            branchless: processing 1 update: ref HEAD
            HEAD is now at 98b9119 create test3.txt
            branchless: processing checkout
            O f777ecc create initial.txt
            |\
            | @ 98b9119 create test3.txt
            | |
            | | & (merge) 96d1c37 create test2.txt
            | |/
            | o 5a6a761 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
            |
            O 62fc20d (master) create test1.txt
            |
            o 96d1c37 create test2.txt
            & (merge) 5a6a761 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
            Successfully rebased and updated detached HEAD.
            branchless: processing 1 update: ref refs/rewritten/parent-3
            "###);
        }

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            O f777ecc create initial.txt
            |\
            | @ 98b9119 create test3.txt
            | |
            | | & (merge) 96d1c37 create test2.txt
            | |/
            | o 5a6a761 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
            |
            O 62fc20d (master) create test1.txt
            |
            o 96d1c37 create test2.txt
            & (merge) 5a6a761 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
            "###);
        }
    }

    // --in-memory
    {
        let git = git.duplicate_repo()?;

        {
            let (stdout, stderr) = git.branchless_with_options(
                "move",
                &[
                    "--in-memory",
                    "--merge",
                    "-s",
                    &test2_oid.to_string(),
                    "-d",
                    "master",
                ],
                &GitRunOptions {
                    expected_exit_code: 2,
                    ..Default::default()
                },
            )?;
            insta::assert_snapshot!(stderr, @r###"
            error: The argument '--in-memory' cannot be used with '--merge'

            Usage: git-branchless move --in-memory --source <SOURCE> --dest <DEST>

            For more information try '--help'
            "###);
            insta::assert_snapshot!(stdout, @"");
        }
    }

    // no flag
    {
        {
            let (stdout, stderr) = git.branchless_with_options(
                "move",
                &["-s", &test2_oid.to_string(), "-d", "master"],
                &GitRunOptions {
                    expected_exit_code: 1,
                    ..Default::default()
                },
            )?;
            insta::assert_snapshot!(stdout, @r###"
            Attempting rebase in-memory...
            Merge commits currently can't be rebased in-memory.
            The merge commit was: 28790c7 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
            To resolve merge conflicts, retry this operation with the --merge option.
            "###);
            insta::assert_snapshot!(stderr, @"");
        }

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            O f777ecc create initial.txt
            |\
            | o fe65c1f create test2.txt
            | & (merge) 28790c7 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
            |\
            | @ 98b9119 create test3.txt
            | |
            | | & (merge) fe65c1f create test2.txt
            | |/
            | o 28790c7 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
            |
            O 62fc20d (master) create test1.txt
            "###);
        }

        // --merge with no other flag
        {
            {
                let (stdout, stderr) = git.branchless(
                    "move",
                    &["--merge", "-s", &test2_oid.to_string(), "-d", "master"],
                )?;
                insta::assert_snapshot!(stdout, @r###"
                Attempting rebase in-memory...
                Failed to merge in-memory, trying again on-disk...
                branchless: running command: <git-executable> diff --quiet
                Calling Git for on-disk rebase...
                branchless: running command: <git-executable> rebase --continue
                "###);
                insta::assert_snapshot!(stderr, @r###"
                branchless: processing 1 update: ref HEAD
                branchless: processing 1 update: ref HEAD
                branchless: processed commit: 96d1c37 create test2.txt
                Executing: git branchless hook-detect-empty-commit fe65c1fe15584744e649b2c79d4cf9b0d878f92e
                branchless: processing 1 update: ref refs/rewritten/parent-3
                branchless: processing 1 update: ref HEAD
                Executing: git branchless hook-register-extra-post-rewrite-hook
                branchless: processing 2 rewritten commits
                branchless: creating working copy snapshot
                branchless: running command: <git-executable> checkout 98b9119d16974f372e76cb64a3b77c528fc0b18b
                Previous HEAD position was 5a6a761 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
                branchless: processing 1 update: ref HEAD
                HEAD is now at 98b9119 create test3.txt
                branchless: processing checkout
                O f777ecc create initial.txt
                |\
                | @ 98b9119 create test3.txt
                | |
                | | & (merge) 96d1c37 create test2.txt
                | |/
                | o 5a6a761 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
                |
                O 62fc20d (master) create test1.txt
                |
                o 96d1c37 create test2.txt
                & (merge) 5a6a761 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
                Successfully rebased and updated detached HEAD.
                branchless: processing 1 update: ref refs/rewritten/parent-3
                "###);
            }

            {
                let stdout = git.smartlog()?;
                insta::assert_snapshot!(stdout, @r###"
                O f777ecc create initial.txt
                |\
                | @ 98b9119 create test3.txt
                | |
                | | & (merge) 96d1c37 create test2.txt
                | |/
                | o 5a6a761 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
                |
                O 62fc20d (master) create test1.txt
                |
                o 96d1c37 create test2.txt
                & (merge) 5a6a761 Merge commit 'fe65c1fe15584744e649b2c79d4cf9b0d878f92e' into HEAD
                "###);
            }
        }
    }

    Ok(())
}

#[test]
fn test_move_merge_commit_both_parents() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    let _test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.run(&["checkout", "HEAD~"])?;
    git.commit_file("test5", 5)?;
    git.run(&["merge", &test4_oid.to_string()])?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        o 70deb1e create test3.txt
        |\
        | o 355e173 create test4.txt
        | & (merge) 8fb706a Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
        |
        o 9ea1b36 create test5.txt
        |
        | & (merge) 355e173 create test4.txt
        |/
        @ 8fb706a Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless(
            "move",
            &["-s", &test3_oid.to_string(), "-d", &test1_oid.to_string()],
        )?;
        // FIXME: this operation should successfully move the merge commit.
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/3] Committed as: 4838e49 create test3.txt
        [2/3] Committed as: a248207 create test4.txt
        [3/3] Committed as: b1f9efa create test5.txt
        branchless: processing 3 rewritten commits
        branchless: This operation abandoned 1 commit!
        branchless: Consider running one of the following:
        branchless:   - git restack: re-apply the abandoned commits/branches
        branchless:     (this is most likely what you want to do)
        branchless:   - git smartlog: assess the situation
        branchless:   - git hide [<commit>...]: hide the commits from the smartlog
        branchless:   - git undo: undo the operation
        hint: disable this hint by running: git config --global branchless.hint.restackWarnAbandoned false
        In-memory rebase succeeded.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 96d1c37 create test2.txt
        | |
        | x 70deb1e (rewritten as 4838e49b) create test3.txt
        | |\
        | | x 355e173 (rewritten as a2482074) create test4.txt
        | | & (merge) 8fb706a Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
        | |
        | x 9ea1b36 (rewritten as b1f9efa0) create test5.txt
        | |
        | | & (merge) 355e173 (rewritten as a2482074) create test4.txt
        | |/
        | @ 8fb706a Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
        |
        o 4838e49 create test3.txt
        |\
        | o a248207 create test4.txt
        |
        o b1f9efa create test5.txt
        hint: there is 1 abandoned commit in your commit graph
        hint: to fix this, run: git restack
        hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
        "###);
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
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 96d1c37 (master) create test2.txt
        "###);
    }

    git.run(&["commit", "-m", "new root"])?;
    {
        let stdout = git.smartlog()?;
        // FIXME: the smartlog handling for unrelated roots is wrong. There
        // should be no relation between these two commits.
        insta::assert_snapshot!(stdout, @r###"
        @ da90168 (> new-root) new root
        :
        O 96d1c37 (master) create test2.txt
        "###);
    }

    git.commit_file("test3", 3)?;

    // --on-disk
    {
        let git = git.duplicate_repo()?;

        {
            let (stdout, stderr) = git.branchless("move", &["--on-disk", "-d", "master"])?;
            insta::assert_snapshot!(stderr, @r###"
            branchless: processing 1 update: ref HEAD
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: 270b681 new root
            Executing: git branchless hook-detect-empty-commit da90168b4835f97f1a10bcc12833140056df9157
            branchless: processing 1 update: ref HEAD
            branchless: processed commit: 70deb1e create test3.txt
            Executing: git branchless hook-detect-empty-commit fc09f3d9f0b7370dc38e761e3730a856dc5025c2
            Executing: git branchless hook-register-extra-post-rewrite-hook
            branchless: processing 3 rewritten commits
            branchless: processing 1 update: branch new-root
            branchless: creating working copy snapshot
            branchless: running command: <git-executable> checkout new-root
            Switched to branch 'new-root'
            branchless: processing checkout
            :
            O 96d1c37 (master) create test2.txt
            |
            @ 70deb1e (> new-root) create test3.txt
            Successfully rebased and updated detached HEAD.
            "###);
            insta::assert_snapshot!(stdout, @r###"
            branchless: running command: <git-executable> diff --quiet
            Calling Git for on-disk rebase...
            branchless: running command: <git-executable> rebase --continue
            Skipped now-empty commit: 270b681 new root
            "###);
        }

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 96d1c37 (master) create test2.txt
            |
            @ 70deb1e (> new-root) create test3.txt
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["branch", "--show-current"])?;
            insta::assert_snapshot!(stdout, @"new-root");
        }
    }

    // --in-memory
    {
        {
            let (stdout, stderr) = git.branchless("move", &["--in-memory", "-d", "master"])?;
            insta::assert_snapshot!(stderr, @r###"
            branchless: creating working copy snapshot
            Previous HEAD position was fc09f3d create test3.txt
            Switched to branch 'new-root'
            branchless: processing checkout
            "###);
            insta::assert_snapshot!(stdout, @r###"
            Attempting rebase in-memory...
            [1/2] Skipped now-empty commit: 270b681 new root
            [2/2] Committed as: 70deb1e create test3.txt
            branchless: processing 1 update: branch new-root
            branchless: processing 2 rewritten commits
            branchless: running command: <git-executable> checkout new-root
            :
            O 96d1c37 (master) create test2.txt
            |
            @ 70deb1e (> new-root) create test3.txt
            In-memory rebase succeeded.
            "###);
        }

        {
            let stdout = git.smartlog()?;
            insta::assert_snapshot!(stdout, @r###"
            :
            O 96d1c37 (master) create test2.txt
            |
            @ 70deb1e (> new-root) create test3.txt
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
        let (stdout, _stderr) =
            git.branchless("move", &["--in-memory", "-s", "HEAD", "-d", "HEAD^"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 96d1c37 create test2.txt
        branchless: processing 1 rewritten commit
        In-memory rebase succeeded.
        "###);
    }

    {
        git.run(&["branch", "foo"])?;
        let (stdout, _stderr) =
            git.branchless("move", &["--in-memory", "-s", "HEAD", "-d", "HEAD^"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 96d1c37 create test2.txt
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
        })?;
        cloned_repo.branchless("init", &["--main-branch", "other-branch"])?;

        let (stdout, _stderr) = cloned_repo.branchless("move", &["-d", "origin/master", "-f"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 70deb1e create test3.txt
        branchless: processing 1 update: branch other-branch
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout other-branch
        Your branch and 'origin/other-branch' have diverged,
        and have 2 and 1 different commits each, respectively.
          (use "git pull" to merge the remote branch into yours)
        :
        @ 70deb1e (> other-branch) create test3.txt
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
        let (stdout, _stderr) = git.branchless_with_options(
            "move",
            &["-d", "original", "--on-disk"],
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
    git.branchless("move", &["-f", "-d", "HEAD^", "--on-disk"])?;
    git.detach_head()?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 96d1c37 (foo, master) create test2.txt
        "###);
    }

    // Get `git reset` to write a new value to `ORIG_HEAD`. If `ORIG_HEAD` is a
    // symbolic reference, it will write to the target reference, rather than
    // overwriting `ORIG_HEAD`.
    git.run(&["reset", "--hard", "HEAD^"])?;
    git.run(&["reset", "--hard", "HEAD^"])?;

    {
        let stdout = git.smartlog()?;
        // `foo` should be unmoved here, rather than moved to
        // `create test1.txt`.
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc create initial.txt
        :
        O 96d1c37 (foo, master) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_move_standalone_create_gc_refs() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo_with_options(&GitInitOptions {
        run_branchless_init: false,
        ..Default::default()
    })?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = git.run(&["show-ref"])?;
        insta::assert_snapshot!(stdout, @"62fc20d2a290daea0d52bdc2ed2ad4be6491010e refs/heads/master
");
    }

    git.branchless("move", &["-d", &test1_oid.to_string()])?;

    {
        let (stdout, _stderr) = git.run(&["show-ref"])?;
        insta::assert_snapshot!(stdout, @r###"
        62fc20d2a290daea0d52bdc2ed2ad4be6491010e refs/branchless/62fc20d2a290daea0d52bdc2ed2ad4be6491010e
        96d1c37a3d4363611c49f7e52186e189a04c531f refs/branchless/96d1c37a3d4363611c49f7e52186e189a04c531f
        62fc20d2a290daea0d52bdc2ed2ad4be6491010e refs/heads/master
        "###);
    }

    Ok(())
}

/// Regression test for <https://github.com/arxanas/git-branchless/issues/249>.
#[test]
fn test_move_branch_on_merge_conflict_resolution() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file_with_contents("test1", 2, "contents 2")?;
    let test3_oid = git.commit_file_with_contents("test1", 3, "contents 3")?;

    git.run(&["checkout", "master"])?;
    git.branchless_with_options(
        "move",
        &["-s", &test3_oid.to_string(), "--merge"],
        &GitRunOptions {
            expected_exit_code: 1,
            ..Default::default()
        },
    )?;

    git.write_file_txt("test1", "contents 3")?;
    git.run(&["add", "."])?;

    {
        let (stdout, stderr) = git.run(&["rebase", "--continue"])?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 1 update: ref HEAD
        branchless: processed commit: 3632ef4 create test1.txt
        Executing: git branchless hook-detect-empty-commit aec59174640c3e3dbb92fdade0bc44ca31552a85
        Executing: git branchless hook-register-extra-post-rewrite-hook
        branchless: processing 1 rewritten commit
        branchless: creating working copy snapshot
        branchless: running command: <git-executable> checkout master
        Previous HEAD position was 3632ef4 create test1.txt
        Switched to branch 'master'
        branchless: processing checkout
        :
        @ 62fc20d (> master) create test1.txt
        |\
        | o 6002762 create test1.txt
        |
        o 3632ef4 create test1.txt
        Successfully rebased and updated detached HEAD.
        "###);
        insta::assert_snapshot!(stdout, @r###"
        [detached HEAD 3632ef4] create test1.txt
         1 file changed, 1 insertion(+), 1 deletion(-)
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d (> master) create test1.txt
        |\
        | o 6002762 create test1.txt
        |
        o 3632ef4 create test1.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["branch", "--show-current"])?;
        insta::assert_snapshot!(stdout, @"master");
    }

    Ok(())
}

#[test]
fn test_move_revset() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.run(&["checkout", "--detach", "master"])?;
    git.commit_file("test4", 4)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test5", 5)?;

    {
        let (stdout, _stderr) = git.branchless("move", &["-s", "draft()", "-d", "master"])?;
        insta::assert_snapshot!(stdout, @r###"
        hint: you can omit the --dest flag in this case, as it defaults to HEAD
        hint: disable this hint by running: git config --global branchless.hint.moveImplicitHeadArgument false
        Attempting rebase in-memory...
        [1/3] Committed as: d895922 create test2.txt
        [2/3] Committed as: f387c23 create test3.txt
        [3/3] Committed as: 9cb6a30 create test4.txt
        branchless: processing 3 rewritten commits
        branchless: running command: <git-executable> checkout master
        :
        @ ea7aa06 (> master) create test5.txt
        |\
        | o d895922 create test2.txt
        | |
        | o f387c23 create test3.txt
        |
        o 9cb6a30 create test4.txt
        In-memory rebase succeeded.
        "###);
    }

    Ok(())
}

#[test]
fn test_move_revset_non_continguous() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    let test4_oid = git.commit_file("test4", 4)?;

    {
        let (stdout, _stderr) = git.branchless(
            "move",
            &["-s", &format!("{test2_oid} | {test4_oid}"), "-d", "master^"],
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/3] Committed as: 8f7aef5 create test4.txt
        [2/3] Committed as: fe65c1f create test2.txt
        [3/3] Committed as: 0206717 create test3.txt
        branchless: processing 3 rewritten commits
        branchless: running command: <git-executable> checkout 8f7aef57d66466a6e0737ae10f67cd98ddecdc66
        O f777ecc create initial.txt
        |\
        | o fe65c1f create test2.txt
        | |
        | o 0206717 create test3.txt
        |\
        | @ 8f7aef5 create test4.txt
        |
        O 62fc20d (master) create test1.txt
        In-memory rebase succeeded.
        "###);
    }

    Ok(())
}

#[test]
fn test_move_hint() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.run(&["checkout", "HEAD^^"])?;

    let hint_command = {
        let git = git.duplicate_repo()?;

        let dest_hint_command = {
            let (stdout, _stderr) =
                git.branchless("move", &["-s", &test3_oid.to_string(), "-d", "."])?;
            insta::assert_snapshot!(stdout, @r###"
        hint: you can omit the --dest flag in this case, as it defaults to HEAD
        hint: disable this hint by running: git config --global branchless.hint.moveImplicitHeadArgument false
        Attempting rebase in-memory...
        [1/1] Committed as: 4838e49 create test3.txt
        branchless: processing 1 rewritten commit
        In-memory rebase succeeded.
        "###);
            extract_hint_command(&stdout)
        };

        let base_hint_command = {
            git.branchless("next", &["--newest"])?;
            let (stdout, _stderr) =
                git.branchless("move", &["-b", ".", "-d", &test2_oid.to_string()])?;
            insta::assert_snapshot!(stdout, @r###"
        hint: you can omit the --base flag in this case, as it defaults to HEAD
        hint: disable this hint by running: git config --global branchless.hint.moveImplicitHeadArgument false
        Attempting rebase in-memory...
        [1/1] Committed as: 70deb1e create test3.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout 70deb1e28791d8e7dd5a1f0c871a51b91282562f
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        @ 70deb1e create test3.txt
        In-memory rebase succeeded.
        "###);
            extract_hint_command(&stdout)
        };

        assert_eq!(base_hint_command, dest_hint_command);
        base_hint_command
    };

    git.run(&hint_command)?;

    {
        let (stdout, _stderr) =
            git.branchless("move", &["-s", &test3_oid.to_string(), "-d", "."])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 4838e49 create test3.txt
        branchless: processing 1 rewritten commit
        In-memory rebase succeeded.
        "###);
    }
    {
        git.branchless("next", &["--newest"])?;
        let (stdout, _stderr) =
            git.branchless("move", &["-b", ".", "-d", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 70deb1e create test3.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout 70deb1e28791d8e7dd5a1f0c871a51b91282562f
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        @ 70deb1e create test3.txt
        In-memory rebase succeeded.
        "###);
    }

    Ok(())
}

#[test]
fn test_move_public_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = git.branchless_with_options(
            "move",
            &["-x", ".^"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        You are trying to rewrite 2 public commits, such as: 96d1c37 create test2.txt
        It is generally not advised to rewrite public commits, because your
        collaborators will have difficulty merging your changes.
        Retry with -f/--force-rewrite to proceed anyways.
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("move", &["-x", ".^", "-f"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/2] Committed as: fe65c1f create test2.txt
        [2/2] Committed as: 0770943 create test1.txt
        branchless: processing 1 update: branch master
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout master
        :
        @ fe65c1f (> master) create test2.txt
        |
        o 0770943 create test1.txt
        In-memory rebase succeeded.
        "###);
    }

    Ok(())
}

#[test]
fn test_move_delete_branch_config_entry() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    {
        git.run(&["remote", "add", "self", "."])?;
    }

    git.run(&["checkout", "-b", "foo"])?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "master", "-b", "bar"])?;
    git.run(&["cherry-pick", "foo^"])?;

    {
        let (stdout, _stderr) = git.run(&["config", "branch.foo.remote", "self"])?;
        insta::assert_snapshot!(stdout, @"");
    }

    {
        let (stdout, _stderr) = git.branchless("move", &["-d", "foo"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Skipped commit (was already applied upstream): 047b7ad create test1.txt
        branchless: processing 1 update: branch bar
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout foo
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 96d1c37 (> foo) create test2.txt
        In-memory rebase succeeded.
        "###);
    }

    {
        let (stdout, stderr) = git.run(&["config", "branch.foo.remote", "--default"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        "###);
    }

    Ok(())
}
