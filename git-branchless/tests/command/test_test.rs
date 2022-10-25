use lib::testing::{make_git, GitRunOptions, GitWrapper};

fn write_test_script(git: &GitWrapper) -> eyre::Result<()> {
    git.write_file(
        "test.sh",
        r#"
for (( i=1; i<="$1"; i++ )); do
    echo "This is line $i"
done
"#,
    )?;
    Ok(())
}

#[test]
fn test_test() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-c", "exit 0"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran exit 0 on 2 commits:
        ✓ Passed: fe65c1f create test2.txt
        ✓ Passed: 0206717 create test3.txt
        2 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, _stderr) = git.run_with_options(
            &["test", "run", "-c", "exit 1"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran exit 1 on 2 commits:
        ✗️ Failed with exit code 1: fe65c1f create test2.txt
        ✗️ Failed with exit code 1: 0206717 create test3.txt
        0 passed, 2 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    Ok(())
}

#[cfg(unix)] // `kill` command doesn't work on Windows.
#[test]
fn test_test_abort() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    {
        let (stdout, _stderr) = git.run_with_options(
            // Kill the parent process (i.e. the owning `git branchless test run` process).
            &["test", "run", "-c", "kill $PPID"],
            &GitRunOptions {
                expected_exit_code: 143,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        "###);
    }

    {
        let (stdout, _stderr) = git.run_with_options(
            &["test", "run", "-c", "exit 0"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        A rebase operation is already in progress.
        Run git rebase --continue or git rebase --abort to resolve it and proceed.
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ fe65c1f create test2.txt
        |
        o 0206717 create test3.txt
        "###);
    }

    git.run(&["rebase", "--abort"])?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o fe65c1f create test2.txt
        |
        @ 0206717 create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_test_cached_results() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.run(&["revert", "HEAD"])?;

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-c", "exit 0"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran exit 0 on 3 commits:
        ✓ Passed: fe65c1f create test2.txt
        ✓ Passed: 0206717 create test3.txt
        ✓ Passed (cached): 1b0d484 Revert "create test3.txt"
        3 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-c", "exit 0"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran exit 0 on 3 commits:
        ✓ Passed (cached): fe65c1f create test2.txt
        ✓ Passed (cached): 0206717 create test3.txt
        ✓ Passed (cached): 1b0d484 Revert "create test3.txt"
        3 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    Ok(())
}

#[cfg(unix)] // Paths don't match on Windows.
#[test]
fn test_test_verbosity() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;

    write_test_script(&git)?;
    let short_command = "bash test.sh 10";
    let long_command = "bash test.sh 15";

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-c", short_command, "-v"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran bash test.sh 10 on 1 commit:
        ✓ Passed: fe65c1f create test2.txt
        Stdout: <repo-path>/.git/branchless/test/bash_test.sh_10/48bb2464c55090a387ed70b3d229705a94856efb/stdout
        This is line 1
        This is line 2
        This is line 3
        This is line 4
        This is line 5
        This is line 6
        This is line 7
        This is line 8
        This is line 9
        This is line 10
        Stderr: <repo-path>/.git/branchless/test/bash_test.sh_10/48bb2464c55090a387ed70b3d229705a94856efb/stderr
        <no output>
        1 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-c", short_command, "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran bash test.sh 10 on 1 commit:
        ✓ Passed (cached): fe65c1f create test2.txt
        Stdout: <repo-path>/.git/branchless/test/bash_test.sh_10/48bb2464c55090a387ed70b3d229705a94856efb/stdout
        This is line 1
        This is line 2
        This is line 3
        This is line 4
        This is line 5
        This is line 6
        This is line 7
        This is line 8
        This is line 9
        This is line 10
        Stderr: <repo-path>/.git/branchless/test/bash_test.sh_10/48bb2464c55090a387ed70b3d229705a94856efb/stderr
        <no output>
        1 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-c", long_command, "-v"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran bash test.sh 15 on 1 commit:
        ✓ Passed: fe65c1f create test2.txt
        Stdout: <repo-path>/.git/branchless/test/bash_test.sh_15/48bb2464c55090a387ed70b3d229705a94856efb/stdout
        This is line 1
        This is line 2
        This is line 3
        This is line 4
        This is line 5
        <5 more lines>
        This is line 11
        This is line 12
        This is line 13
        This is line 14
        This is line 15
        Stderr: <repo-path>/.git/branchless/test/bash_test.sh_15/48bb2464c55090a387ed70b3d229705a94856efb/stderr
        <no output>
        1 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-c", long_command, "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran bash test.sh 15 on 1 commit:
        ✓ Passed (cached): fe65c1f create test2.txt
        Stdout: <repo-path>/.git/branchless/test/bash_test.sh_15/48bb2464c55090a387ed70b3d229705a94856efb/stdout
        This is line 1
        This is line 2
        This is line 3
        This is line 4
        This is line 5
        This is line 6
        This is line 7
        This is line 8
        This is line 9
        This is line 10
        This is line 11
        This is line 12
        This is line 13
        This is line 14
        This is line 15
        Stderr: <repo-path>/.git/branchless/test/bash_test.sh_15/48bb2464c55090a387ed70b3d229705a94856efb/stderr
        <no output>
        1 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    Ok(())
}
