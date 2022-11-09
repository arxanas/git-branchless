use crate::util::extract_hint_command;
use lib::testing::{
    make_git, make_git_with_remote_repo, GitInitOptions, GitRunOptions, GitWrapperWithRemoteRepo,
};

#[test]
fn test_init_smartlog() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | o 62fc20d (test1) create test1.txt
        | |
        | @ fa4e4e1 (> test2and3) Merge branch 'test1' into test2and3
        |
        o fe65c1f create test2.txt
        |
        o 0206717 create test3.txt
        |
        @ fa4e4e1 (> test2and3) Merge branch 'test1' into test2and3
        "###);
    }

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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
    git.run(&["prev"])?;
    git.run(&["commit", "--amend", "-m", "test1 version 1"])?;
    git.run(&["commit", "--amend", "-m", "test1 version 2"])?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_show_hidden_commits() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["commit", "--amend", "-m", "amended test2"])?;
    let test2_oid_amended = git.get_repo()?.get_head_info()?.oid.unwrap();
    git.run(&["hide", "HEAD"])?;
    git.run(&["checkout", "HEAD^"])?;

    {
        let (stdout, stderr) = git.run(&[
            "smartlog",
            "--hidden",
            &format!("{test1_oid} + {test2_oid} + {test2_oid_amended}"),
        ])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d (master) create test1.txt
        |\
        | x cb8137a (manually hidden) amended test2
        |
        x 96d1c37 (rewritten as cb8137ad) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_show_only_branches() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test3", 3)?;
    git.detach_head()?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test5", 5)?;
    git.detach_head()?;
    let test6_oid = git.commit_file("test6", 6)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test7", 7)?;
    git.detach_head()?;
    git.commit_file("test8", 8)?;
    git.run(&["checkout", "master"])?;
    git.commit_file("test9", 9)?;

    git.run(&["branch", "branch-2", &test2_oid.to_string()])?;
    git.run(&["branch", "branch-4", &test4_oid.to_string()])?;
    git.run(&["hide", &test4_oid.to_string()])?;
    git.run(&["hide", &test6_oid.to_string()])?;

    // confirm our baseline:
    // branch, hidden branch and non-branch head are visible; hidden non-branch head is not
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d create test1.txt
        |\
        | o 96d1c37 (branch-2) create test2.txt
        |
        O 4838e49 create test3.txt
        |\
        : x a248207 (manually hidden) (branch-4) create test4.txt
        :
        O 8577a96 create test7.txt
        |\
        | o e8b6a38 create test8.txt
        |
        @ 1b854ed (> master) create test9.txt
        "###);
    }

    // just branches (normal and hidden) but no non-branch heads
    {
        let (stdout, _stderr) = git.run(&["smartlog", "branches()"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d create test1.txt
        |\
        | o 96d1c37 (branch-2) create test2.txt
        |
        O 4838e49 create test3.txt
        |\
        : x a248207 (manually hidden) (branch-4) create test4.txt
        :
        @ 1b854ed (> master) create test9.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_active_non_head_main_branch_commit() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;

    let test1_oid = {
        original_repo.init_repo()?;
        let test1_oid = original_repo.commit_file("test1", 1)?;
        original_repo.commit_file("test2", 2)?;
        original_repo.commit_file("test3", 3)?;

        original_repo.clone_repo_into(&cloned_repo, &[])?;

        test1_oid
    };

    {
        cloned_repo.init_repo_with_options(&GitInitOptions {
            make_initial_commit: false,
            ..Default::default()
        })?;
        // Ensure that the `test1` commit isn't visible just because it's been
        // un-hidden. It's a public commit, so it should be hidden if possible.
        cloned_repo.run(&["unhide", &test1_oid.to_string()])?;

        let (stdout, _stderr) = cloned_repo.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 70deb1e (> master) create test3.txt
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog", "none()"])?;
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
        let (stdout, _stderr) = git.run(&["smartlog", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
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
        let (stdout, _stderr) = git.run(&["smartlog", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
        | :
        | o 96d1c37 create test2.txt
        | :
        |
        O 8f7aef5 (master) create test4.txt
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
        let (stdout, _stderr) = git.run(&["smartlog", "none()"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d (master) create test1.txt
        :
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
        let (stdout, _stderr) = git.run(&["smartlog", "--hidden"])?;
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
