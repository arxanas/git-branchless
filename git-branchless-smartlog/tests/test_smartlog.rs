use lib::testing::{GitRunOptions, extract_hint_command, make_git};

#[test]
fn test_init_smartlog() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @"@ f777ecc (> master) create initial.txt
");
    }

    Ok(())
}

#[test]
fn test_show_reachable_commit() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["checkout", "-b", "initial-branch", "master"])?;
    git.commit_file("test", 1)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 3df4b93 (> initial-branch) create test.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_tree() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.run(&["branch", "initial"])?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "initial"])?;
    git.commit_file("test2", 2)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | o 62fc20d create test1.txt
        |
        @ fe65c1f (> initial) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_rebase() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["checkout", "-b", "test1", "master"])?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "master"])?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.run(&["rebase", "test1"])?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d (test1) create test1.txt
        |
        @ f8d9985 create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_sequential_master_commits() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 70deb1e (> master) create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_merge_commit() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["checkout", "-b", "test1", "master"])?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "-b", "test2and3", "master"])?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.run_with_options(
        &["merge", "test1"],
        &GitRunOptions {
            time: 4,
            ..Default::default()
        },
    )?;

    {
        // Rendering here is arbitrary and open to change.
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | o 62fc20d (test1) create test1.txt
        | & (merge) fa4e4e1 (> test2and3) Merge branch 'test1' into test2and3
        |
        o fe65c1f create test2.txt
        |
        o 0206717 create test3.txt
        |
        | & (merge) 62fc20d (test1) create test1.txt
        |/
        @ fa4e4e1 (> test2and3) Merge branch 'test1' into test2and3
        "###);
    }

    git.run(&["checkout", "-b", "test4", "master"])?;
    git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;
    git.run(&["merge", "test1", "test2and3"])?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | o 62fc20d (test1) create test1.txt
        | & (merge) fa4e4e1 (test2and3) Merge branch 'test1' into test2and3
        |\
        | o fe65c1f create test2.txt
        | |
        | o 0206717 create test3.txt
        | |
        | | & (merge) 62fc20d (test1) create test1.txt
        | |/
        | o fa4e4e1 (test2and3) Merge branch 'test1' into test2and3
        | & (merge) 36a25e8 (> test4) Merge branch 'test2and3' into test4
        |
        o 8f7aef5 create test4.txt
        |
        o 47d30fa create test5.txt
        |
        | & (merge) fa4e4e1 (test2and3) Merge branch 'test1' into test2and3
        |/
        @ 36a25e8 (> test4) Merge branch 'test2and3' into test4
        "###);
    }

    Ok(())
}

#[test]
fn test_merge_commit_reverse_order() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["config", "branchless.smartlog.reverse", "true"])?;
    git.run(&["checkout", "-b", "test1", "master"])?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "-b", "test2and3", "master"])?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.run_with_options(
        &["merge", "test1"],
        &GitRunOptions {
            time: 4,
            ..Default::default()
        },
    )?;

    let (stdout, _) = git.branchless("smartlog", &[])?;
    insta::assert_snapshot!(stdout, @r###"
    @ fa4e4e1 (> test2and3) Merge branch 'test1' into test2and3
    |\
    | & (merge) 62fc20d (test1) create test1.txt
    |
    o 0206717 create test3.txt
    |
    o fe65c1f create test2.txt
    |
    | & (merge) fa4e4e1 (> test2and3) Merge branch 'test1' into test2and3
    | o 62fc20d (test1) create test1.txt
    |/
    O f777ecc (master) create initial.txt
    "###);

    Ok(())
}

#[test]
fn test_rebase_conflict() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["checkout", "-b", "branch1", "master"])?;
    git.commit_file_with_contents("test", 1, "contents 1\n")?;
    git.run(&["checkout", "-b", "branch2", "master"])?;
    git.commit_file_with_contents("test", 2, "contents 2\n")?;

    // Should produce a conflict.
    git.run_with_options(
        &["rebase", "branch1"],
        &GitRunOptions {
            expected_exit_code: 1,
            ..Default::default()
        },
    )?;
    git.resolve_file("test", "contents resolved\n")?;
    git.run(&["rebase", "--continue"])?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 88646b5 (branch1) create test.txt
        |
        @ 4549af3 (> branch2) create test.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_non_adjacent_commits() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.detach_head()?;
    git.commit_file("test4", 4)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
        : o 62fc20d create test1.txt
        :
        O 0206717 (master) create test3.txt
        |
        @ 8e62740 create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_non_adjacent_commits2() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;
    git.detach_head()?;
    git.commit_file("test5", 5)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
        : o 62fc20d create test1.txt
        : |
        : o 96d1c37 create test2.txt
        :
        O 2b633ed (master) create test4.txt
        |
        @ 1393298 create test5.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_non_adjacent_commits3() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test3", 3)?;
    git.detach_head()?;
    git.commit_file("test4", 4)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test5", 5)?;
    git.commit_file("test6", 6)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d create test1.txt
        |\
        | o 96d1c37 create test2.txt
        |
        O 4838e49 create test3.txt
        |\
        : o a248207 create test4.txt
        :
        @ 500c9b3 (> master) create test6.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_custom_main_branch() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["branch", "-m", "master", "main"])?;
    git.run(&["config", "branchless.core.mainBranch", "main"])?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (main) create test1.txt
        |
        @ 96d1c37 create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_show_rewritten_commit_hash() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD^"])?;
    git.run(&["commit", "--amend", "-m", "test1 version 1"])?;
    git.run(&["commit", "--amend", "-m", "test1 version 2"])?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | @ 2ebe095 test1 version 2
        |
        x 62fc20d (rewritten as 2ebe0950) create test1.txt
        |
        o 96d1c37 create test2.txt
        hint: there is 1 abandoned commit in your commit graph
        hint: to fix this, run: git restack
        hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
        "###);
    }

    Ok(())
}

#[test]
fn test_smartlog_orphaned_root() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;

    git.run(&["checkout", "--orphan", "new-root"])?;

    {
        let (stdout, stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_smartlog_hint_abandoned() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD^"])?;
    git.run(&["commit", "--amend", "-m", "amended test1"])?;

    let hint_command = {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | @ ae94dc2 amended test1
        |
        x 62fc20d (rewritten as ae94dc2a) create test1.txt
        |
        o 96d1c37 create test2.txt
        hint: there is 1 abandoned commit in your commit graph
        hint: to fix this, run: git restack
        hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
        "###);
        extract_hint_command(&stdout)
    };

    git.run(&hint_command)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | @ ae94dc2 amended test1
        |
        x 62fc20d (rewritten as ae94dc2a) create test1.txt
        |
        o 96d1c37 create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_smartlog_hint_abandoned_except_current_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    let test1_oid = git.commit_file("test1", 1)?;
    git.run(&["commit", "--amend", "--message", "amended test1"])?;
    git.run(&["checkout", &test1_oid.to_string()])?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
        | % 62fc20d (rewritten as ae94dc2a) create test1.txt
        |
        O ae94dc2 (master) amended test1
        "###);
    }

    Ok(())
}

/// When branchless.smartlog.reverse is `true`, hints still appear at the end of output
#[test]
fn test_smartlog_hint_abandoned_reverse_order() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;
    git.run(&["config", "branchless.smartlog.reverse", "true"])?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD^"])?;
    git.run(&["commit", "--amend", "-m", "amended test1"])?;

    let (stdout, _) = git.branchless("smartlog", &[])?;
    insta::assert_snapshot!(stdout, @r###"
    o 96d1c37 create test2.txt
    |
    x 62fc20d (rewritten as ae94dc2a) create test1.txt
    |
    | @ ae94dc2 amended test1
    |/
    O f777ecc (master) create initial.txt
    hint: there is 1 abandoned commit in your commit graph
    hint: to fix this, run: git restack
    hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
    "###);

    Ok(())
}

#[test]
fn test_smartlog_sparse() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.detach_head()?;
    git.commit_file("test4", 4)?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &["none()"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 0206717 (master) create test3.txt
        |
        @ 8e62740 create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_smartlog_sparse_branch() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;
    git.detach_head()?;
    git.commit_file("test5", 5)?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[&test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
        : # 1 omitted commit
        : :
        : o 96d1c37 create test2.txt
        :
        O 2b633ed (master) create test4.txt
        |
        @ 1393298 create test5.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_smartlog_sparse_false_head() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test4", 4)?;
    git.detach_head()?;
    git.commit_file("test5", 5)?;
    git.commit_file("test6", 6)?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[&test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
        | # 1 omitted commit
        | :
        | o 96d1c37 create test2.txt
        | :
        | # 1 omitted descendant commit
        |
        O 8f7aef5 (master) create test4.txt
        :
        # 1 omitted commit
        :
        @ 68975e5 create test6.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_smartlog_sparse_main_false_head() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD~"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &["none()"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d (master) create test1.txt
        :
        # 1 omitted descendant commit
        "###);
    }

    Ok(())
}

#[test]
fn test_smartlog_hidden() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.run(&["commit", "--amend", "-m", "amended test1"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &["--hidden"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | @ ae94dc2 amended test1
        |
        x 62fc20d (rewritten as ae94dc2a) create test1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_smartlog_sparse_vertical_ellipsis_sibling_commits() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD~"])?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD~"])?;
    git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &["heads(draft())"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | o 62fc20d create test1.txt
        |\
        | o fe65c1f create test2.txt
        :
        # 1 omitted commit
        :
        @ 2b633ed create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_default_smartlog_revset() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    {
        let smartlog = git.smartlog()?;
        insta::assert_snapshot!(smartlog, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        @ 70deb1e create test3.txt
        "###);
    }

    git.run(&["config", "branchless.smartlog.defaultRevset", "none()"])?;

    {
        let smartlog = git.smartlog()?;
        insta::assert_snapshot!(smartlog, @r###"
        O f777ecc (master) create initial.txt
        :
        # 2 omitted commits
        :
        @ 70deb1e create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_exact() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test5", 5)?;
    git.detach_head()?;
    git.commit_file("test6", 6)?;
    git.run(&["checkout", "96d1c37"])?;

    {
        // Here '--exact' doesn't change anything because draft() covers all these commits
        let (stdout, _stderr) = git.branchless("smartlog", &["--exact"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d create test1.txt
        |\
        | @ 96d1c37 create test2.txt
        | |
        | o 70deb1e create test3.txt
        | |
        | o 355e173 create test4.txt
        |
        O ea7aa06 (master) create test5.txt
        |
        o da42aeb create test6.txt
        "###);
    }

    {
        // Show no commits
        let (stdout, _stderr) = git.branchless("smartlog", &["--exact", "none()"])?;
        insta::assert_snapshot!(stdout, @"");
    }

    {
        // Show one commit - no master or HEAD
        let (stdout, _stderr) = git.branchless("smartlog", &["--exact", "70deb1e"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        o 70deb1e create test3.txt
        :
        # 1 omitted descendant commit
        "###);
    }

    {
        // Show head commits and their common ancestor, which is not main.
        let (stdout, _stderr) = git.branchless("smartlog", &["--exact", "heads(draft())"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d create test1.txt
        |\
        : # 2 omitted commits
        : :
        : o 355e173 create test4.txt
        :
        o da42aeb create test6.txt
        "###);
    }

    Ok(())
}
