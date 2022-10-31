use lib::testing::{make_git, GitInitOptions, GitRunOptions};

/// Remove some of the output from `git rebase`, as it seems to be
/// non-deterministic as to whether or not it appears.
pub fn remove_rebase_lines(output: String) -> String {
    output
        .lines()
        .filter(|line| !line.contains("First, rewinding head") && !line.contains("Applying:"))
        .filter(|line| {
            // See https://github.com/arxanas/git-branchless/issues/87.  Before
            // Git v2.33 (`next` branch), the "Auto-merging" line appears
            // *after* the "CONFLICT" line for a given file (which doesn't make
            // sense -- how can there be a conflict before merging has started)?
            // The development version of Git v2.33 fixes this and places the
            // "Auto-merging" line *before* the "CONFLICT" line. To avoid having
            // to deal with multiple possible output formats, just remove the
            // line in question.
            !line.contains("Auto-merging")
        })
        .map(|line| format!("{}\n", line))
        .collect()
}

#[test]
fn test_restack_amended_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }

    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.run(&["checkout", "HEAD^^"])?;
    git.run(&["commit", "--amend", "-m", "amend test1.txt"])?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | @ 024c35c amend test1.txt
        |
        x 62fc20d (rewritten as 024c35ce) create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        o 70deb1e create test3.txt
        hint: there is 1 abandoned commit in your commit graph
        hint: to fix this, run: git restack
        hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["restack", "--on-disk"])?;
        let stdout = remove_rebase_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Finished restacking commits.
        No abandoned branches to restack.
        O f777ecc (master) create initial.txt
        |
        @ 024c35c amend test1.txt
        |
        o 8cd7de6 create test2.txt
        |
        o b9a0491 create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_restack_consecutive_rewrites() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }

    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.run(&["checkout", "HEAD^^"])?;
    git.run(&["commit", "--amend", "-m", "amend test1.txt v1"])?;
    git.run(&["commit", "--amend", "-m", "amend test1.txt v2"])?;

    {
        let (stdout, _stderr) = git.run(&["restack", "--on-disk"])?;
        let stdout = remove_rebase_lines(stdout);

        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Finished restacking commits.
        No abandoned branches to restack.
        O f777ecc (master) create initial.txt
        |
        @ 662b451 amend test1.txt v2
        |
        o 8e9bbde create test2.txt
        |
        o 9dc6dd0 create test3.txt
        "###)
    }

    Ok(())
}

#[test]
fn test_move_abandoned_branch() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.run(&["commit", "--amend", "-m", "amend test1.txt v1"])?;
    git.run(&["commit", "--amend", "-m", "amend test1.txt v2"])?;

    {
        let (stdout, _stderr) = git.run(&["restack"])?;
        let stdout = remove_rebase_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        No abandoned commits to restack.
        branchless: processing 1 update: branch master
        Finished restacking branches.
        :
        @ 662b451 (master) amend test1.txt v2
        "###);
    }

    Ok(())
}

#[test]
fn test_amended_initial_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }

    git.init_repo()?;
    git.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

    git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD^"])?;
    git.run(&["commit", "--amend", "-m", "new initial commit"])?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        @ 9a9f929 new initial commit
        :
        O 62fc20d (master) create test1.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["restack", "--on-disk", "--force-rewrite"])?;
        let stdout = remove_rebase_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Finished restacking commits.
        No abandoned branches to restack.
        @ 9a9f929 new initial commit
        |
        O 6d85943 (master) create test1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_restack_amended_master() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }

    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.detach_head()?;
    git.run(&["checkout", "HEAD^"])?;
    git.run(&["commit", "--amend", "-m", "amended test1"])?;

    {
        let (stdout, _stderr) = git.run(&["restack", "--on-disk", "--force-rewrite"])?;
        let stdout = remove_rebase_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Finished restacking commits.
        No abandoned branches to restack.
        :
        @ ae94dc2 amended test1
        |
        O 51452b5 (master) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_restack_aborts_during_rebase_conflict() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.run(&["branch", "foo"])?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["prev"])?;

    git.write_file_txt("test2", "conflicting test2 contents")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "--amend", "-m", "amend test1 with test2 conflict"])?;

    {
        let (stdout, _stderr) = git.run_with_options(
            &["restack", "-f"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        This operation would cause a merge conflict:
        - (1 conflicting file) 96d1c37 create test2.txt
        To resolve merge conflicts, retry this operation with the --merge option.
        "###);
    }

    {
        let (stdout, _stderr) = git.run_with_options(
            &["restack", "--force-rewrite", "--merge"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        let stdout = remove_rebase_lines(stdout);

        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        There was a merge conflict, which currently can't be resolved when rebasing in-memory.
        The conflicting commit was: 96d1c37 create test2.txt
        Trying again on-disk...
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        CONFLICT (add/add): Merge conflict in test2.txt
        Error: Could not restack commits (exit code 1).
        You can resolve the error and try running `git restack` again.
        "###);
    }

    Ok(())
}

#[test]
fn test_restack_multiple_amended() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;

    git.run(&["checkout", "HEAD~"])?;
    git.run(&["commit", "--amend", "-m", "test3 amended"])?;
    git.run(&["checkout", "HEAD~"])?;
    git.run(&["commit", "--amend", "-m", "test2 amended"])?;
    git.run(&["checkout", "HEAD~"])?;

    {
        let (stdout, _stderr) = git.run(&["restack", "--on-disk"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Finished restacking commits.
        No abandoned branches to restack.
        O f777ecc (master) create initial.txt
        |
        @ 62fc20d create test1.txt
        |
        o 22f3928 test2 amended
        |
        o 8e06b96 test3 amended
        |
        o f5644e3 create test4.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_restack_single_of_many_commits() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.run(&["checkout", &test1_oid.to_string()])?;
    let test4_oid = git.commit_file("test4", 4)?;
    git.commit_file("test5", 5)?;

    git.run(&["checkout", &test2_oid.to_string()])?;
    git.run(&["commit", "--amend", "-m", "updated test2"])?;

    git.run(&["checkout", &test4_oid.to_string()])?;
    git.run(&["commit", "--amend", "-m", "updated test4"])?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | @ 3bd716d updated test4
        |\
        | o 7357d2b updated test2
        |\
        | x 96d1c37 (rewritten as 7357d2b7) create test2.txt
        | |
        | o 70deb1e create test3.txt
        |
        x bf0d52a (rewritten as 3bd716d5) create test4.txt
        |
        o 848121c create test5.txt
        hint: there are 2 abandoned commits in your commit graph
        hint: to fix this, run: git restack
        hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
        "###);
    }

    {
        let (stdout, stderr) = git.run(&["restack", "--on-disk", &test2_oid.to_string()])?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 1 update: ref HEAD
        branchless: processing 1 update: ref HEAD
        branchless: processed commit: 944f78d create test3.txt
        Executing: git branchless hook-detect-empty-commit 70deb1e28791d8e7dd5a1f0c871a51b91282562f
        Executing: git branchless hook-register-extra-post-rewrite-hook
        branchless: processing 1 rewritten commit
        branchless: creating working copy snapshot
        branchless: running command: <git-executable> checkout 3bd716d57489779ab1daf446f80e66e90b56ead7
        Previous HEAD position was 944f78d create test3.txt
        branchless: processing 1 update: ref HEAD
        HEAD is now at 3bd716d updated test4
        branchless: processing checkout
        :
        O 62fc20d (master) create test1.txt
        |\
        | @ 3bd716d updated test4
        |\
        | o 7357d2b updated test2
        | |
        | o 944f78d create test3.txt
        |
        x bf0d52a (rewritten as 3bd716d5) create test4.txt
        |
        o 848121c create test5.txt
        hint: there is 1 abandoned commit in your commit graph
        hint: to fix this, run: git restack
        hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
        Successfully rebased and updated detached HEAD.
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Finished restacking commits.
        No abandoned branches to restack.
        :
        O 62fc20d (master) create test1.txt
        |\
        | @ 3bd716d updated test4
        |\
        | o 7357d2b updated test2
        | |
        | o 944f78d create test3.txt
        |
        x bf0d52a (rewritten as 3bd716d5) create test4.txt
        |
        o 848121c create test5.txt
        hint: there is 1 abandoned commit in your commit graph
        hint: to fix this, run: git restack
        hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
        "###);
    }

    Ok(())
}

/// Regression test for: https://github.com/arxanas/git-branchless/issues/209
#[test]
fn test_restack_unobserved_commit() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo_with_options(&GitInitOptions {
        run_branchless_init: false,
        ..Default::default()
    })?;

    git.commit_file("test1", 1)?;
    git.run(&["checkout", "-b", "foo"])?;
    git.commit_file("test2", 2)?;
    git.detach_head()?;
    git.commit_file("test3", 3)?;

    git.run(&["branchless", "init"])?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o 96d1c37 (foo) create test2.txt
        |
        @ 70deb1e create test3.txt
        "###);
    }

    // Only abandon `test3` now that it's been observed by the DAG.
    git.run(&["checkout", "foo"])?;

    git.run(&["commit", "--amend", "-m", "Updated test2"])?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        @ f4229de (> foo) Updated test2
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["restack"])?;
        insta::assert_snapshot!(stdout, @r###"
        No abandoned commits to restack.
        No abandoned branches to restack.
        :
        O 62fc20d (master) create test1.txt
        |
        @ f4229de (> foo) Updated test2
        "###);
    }

    Ok(())
}

/// Regression test for https://github.com/arxanas/git-branchless/issues/236
#[test]
fn test_restack_checked_out_branch() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["branch", "foo"])?;
    git.run(&["checkout", "HEAD^"])?;
    git.run(&["commit", "--amend", "-m", "test1 amended"])?;
    git.run(&["checkout", "foo"])?;

    {
        let (stdout, _stderr) = git.run(&["restack", "-f"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 59e7581 create test2.txt
        branchless: processing 2 updates: branch foo, branch master
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout foo
        In-memory rebase succeeded.
        Finished restacking commits.
        No abandoned branches to restack.
        :
        @ 59e7581 (> foo, master) create test2.txt
        "###);
    }

    Ok(())
}

/// Regression test for https://github.com/arxanas/git-branchless/issues/325
#[test]
fn test_restack_non_observed_branch_commit() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["branch", "foo"])?;
    git.run(&["checkout", "HEAD^"])?;
    git.clear_event_log()?;
    git.run(&["commit", "--amend", "-m", "test1 amended"])?;
    git.run(&["checkout", "foo"])?;

    {
        let (stdout, _stderr) = git.run(&["restack", "-f"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 59e7581 create test2.txt
        branchless: processing 2 updates: branch foo, branch master
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout foo
        In-memory rebase succeeded.
        Finished restacking commits.
        No abandoned branches to restack.
        :
        @ 59e7581 (> foo, master) create test2.txt
        "###);
    }

    git.run(&["reset", "--hard", &test2_oid.to_string()])?;
    {
        let (stdout, _stderr) = git.run(&["sl"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
        : x 62fc20d (rewritten as 14042005) create test1.txt
        : |
        : % 96d1c37 (rewritten as 59e75818) (> foo) create test2.txt
        :
        O 59e7581 (master) create test2.txt
        "###);
    }

    {
        // If we didn't enable `branchless.restack.preserveTimestamps`, then
        // this would have created another commit and moved `foo` to it. (Below,
        // it just moves `foo`.) It's not clear what the best behavior here is;
        // the user might not expect the restack operation to create duplicates
        // of all descendant commits just because a branch pointed to one of
        // those descendants.
        let (stdout, _stderr) = git.run(&["restack", "-f"])?;
        insta::assert_snapshot!(stdout, @r###"
        No abandoned commits to restack.
        branchless: processing 1 update: branch foo
        Finished restacking branches.
        :
        @ 59e7581 (> foo, master) create test2.txt
        "###);
    }

    Ok(())
}
