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
        let (stdout, _stderr) = git.run(&["test", "run", "-x", "exit 0"])?;
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
            &["test", "run", "-x", "exit 1"],
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
        X Failed with exit code 1: fe65c1f create test2.txt
        X Failed with exit code 1: 0206717 create test3.txt
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
            &["test", "run", "-x", "kill $PPID"],
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
            &["test", "run", "-x", "exit 0"],
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
        let (stdout, _stderr) = git.run(&["test", "run", "-x", "exit 0"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran exit 0 on 3 commits:
        ✓ Passed: fe65c1f create test2.txt
        ✓ Passed: 0206717 create test3.txt
        ✓ Passed (cached): 1b0d484 Revert "create test3.txt"
        3 passed, 0 failed, 0 skipped
        hint: There was 1 cached test result.
        hint: To clear all cached results, run: git test clean
        hint: disable this hint by running: git config --global branchless.hint.cleanCachedTestResults false
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-x", "exit 0"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran exit 0 on 3 commits:
        ✓ Passed (cached): fe65c1f create test2.txt
        ✓ Passed (cached): 0206717 create test3.txt
        ✓ Passed (cached): 1b0d484 Revert "create test3.txt"
        3 passed, 0 failed, 0 skipped
        hint: There were 3 cached test results.
        hint: To clear all cached results, run: git test clean
        hint: disable this hint by running: git config --global branchless.hint.cleanCachedTestResults false
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
        let (stdout, _stderr) = git.run(&["test", "run", "-x", short_command, "-v"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran bash test.sh 10 on 1 commit:
        ✓ Passed: fe65c1f create test2.txt
        Stdout: <repo-path>/.git/branchless/test/48bb2464c55090a387ed70b3d229705a94856efb/bash_test.sh_10/stdout
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
        Stderr: <repo-path>/.git/branchless/test/48bb2464c55090a387ed70b3d229705a94856efb/bash_test.sh_10/stderr
        <no output>
        1 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-x", short_command, "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran bash test.sh 10 on 1 commit:
        ✓ Passed (cached): fe65c1f create test2.txt
        Stdout: <repo-path>/.git/branchless/test/48bb2464c55090a387ed70b3d229705a94856efb/bash_test.sh_10/stdout
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
        Stderr: <repo-path>/.git/branchless/test/48bb2464c55090a387ed70b3d229705a94856efb/bash_test.sh_10/stderr
        <no output>
        1 passed, 0 failed, 0 skipped
        hint: There was 1 cached test result.
        hint: To clear all cached results, run: git test clean
        hint: disable this hint by running: git config --global branchless.hint.cleanCachedTestResults false
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-x", long_command, "-v"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran bash test.sh 15 on 1 commit:
        ✓ Passed: fe65c1f create test2.txt
        Stdout: <repo-path>/.git/branchless/test/48bb2464c55090a387ed70b3d229705a94856efb/bash_test.sh_15/stdout
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
        Stderr: <repo-path>/.git/branchless/test/48bb2464c55090a387ed70b3d229705a94856efb/bash_test.sh_15/stderr
        <no output>
        1 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-x", long_command, "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran bash test.sh 15 on 1 commit:
        ✓ Passed (cached): fe65c1f create test2.txt
        Stdout: <repo-path>/.git/branchless/test/48bb2464c55090a387ed70b3d229705a94856efb/bash_test.sh_15/stdout
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
        Stderr: <repo-path>/.git/branchless/test/48bb2464c55090a387ed70b3d229705a94856efb/bash_test.sh_15/stderr
        <no output>
        1 passed, 0 failed, 0 skipped
        hint: There was 1 cached test result.
        hint: To clear all cached results, run: git test clean
        hint: disable this hint by running: git config --global branchless.hint.cleanCachedTestResults false
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    Ok(())
}

#[test]
fn test_test_show() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-x", "echo hi", "."])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran echo hi on 1 commit:
        ✓ Passed: 96d1c37 create test2.txt
        1 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, stderr) = git.run(&["test", "show", "-x", "echo hi"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        No cached test data for 62fc20d create test1.txt
        ✓ Passed (cached): 96d1c37 create test2.txt
        hint: To see more detailed output, re-run with -v/--verbose.
        hint: disable this hint by running: git config --global branchless.hint.testShowVerbose false
        "###);
    }

    {
        let (stdout, stderr) = git.run(&["test", "clean"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Cleaning results for 62fc20d create test1.txt
        Cleaning results for 96d1c37 create test2.txt
        Cleaned 2 cached test results.
        "###);
    }

    {
        let (stdout, stderr) = git.run(&["test", "show", "-x", "echo hi"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        No cached test data for 62fc20d create test1.txt
        No cached test data for 96d1c37 create test2.txt
        hint: To see more detailed output, re-run with -v/--verbose.
        hint: disable this hint by running: git config --global branchless.hint.testShowVerbose false
        "###);
    }

    Ok(())
}

#[test]
fn test_test_command_alias() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    {
        let (stdout, _stderr) = git.run_with_options(
            &["test", "run"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Could not determine test command to run. No test command was provided with -c/--command or
        -x/--exec, and the configuration value 'branchless.test.alias.default' was not set.

        To configure a default test command, run: git config branchless.test.alias.default <command>
        To run a specific test command, run: git test run -x <command>
        To run a specific command alias, run: git test run -c <alias>
        "###);
    }

    git.run(&["config", "branchless.test.alias.foo", "echo foo"])?;
    {
        let (stdout, _stderr) = git.run_with_options(
            &["test", "run"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Could not determine test command to run. No test command was provided with -c/--command or
        -x/--exec, and the configuration value 'branchless.test.alias.default' was not set.

        To configure a default test command, run: git config branchless.test.alias.default <command>
        To run a specific test command, run: git test run -x <command>
        To run a specific command alias, run: git test run -c <alias>

        These are the currently-configured command aliases:
        - branchless.test.alias.foo = "echo foo"
        "###);
    }

    {
        let (stdout, _stderr) = git.run_with_options(
            &["test", "run", "-c", "nonexistent"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        The test command alias "nonexistent" was not defined.

        To create it, run: git config branchless.test.alias.nonexistent <command>
        Or use the -x/--exec flag instead to run a test command without first creating an alias.

        These are the currently-configured command aliases:
        - branchless.test.alias.foo = "echo foo"
        "###);
    }

    git.run(&["config", "branchless.test.alias.default", "echo default"])?;
    {
        let (stdout, _stderr) = git.run(&["test", "run"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran echo default on 0 commits:
        0 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["test", "run", "-c", "foo"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran echo foo on 0 commits:
        0 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, _stderr) = git.run_with_options(
            &["test", "run", "-c", "foo bar baz"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        The test command alias "foo bar baz" was not defined.

        To create it, run: git config branchless.test.alias.foo bar baz <command>
        Or use the -x/--exec flag instead to run a test command without first creating an alias.

        These are the currently-configured command aliases:
        - branchless.test.alias.foo = "echo foo"
        - branchless.test.alias.default = "echo default"
        "###);
    }

    Ok(())
}

#[cfg(unix)] // Paths don't match on Windows.
#[test]
fn test_test_worktree_strategy() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.write_file_txt("test1", "Updated contents\n")?;

    {
        let (stdout, stderr) = git.run_with_options(
            &[
                "test",
                "run",
                "--strategy",
                "working-copy",
                "-x",
                "echo hello",
                "@",
            ],
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

    {
        let (stdout, stderr) = git.run(&[
            "test",
            "run",
            "--strategy",
            "worktree",
            "-x",
            "echo hello",
            "-vv",
            "@",
        ])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Ran echo hello on 1 commit:
        ✓ Passed: 62fc20d create test1.txt
        Stdout: <repo-path>/.git/branchless/test/8108c01b1930423879f106c1ebf725fcbfedccda/echo_hello/stdout
        hello
        Stderr: <repo-path>/.git/branchless/test/8108c01b1930423879f106c1ebf725fcbfedccda/echo_hello/stderr
        <no output>
        1 passed, 0 failed, 0 skipped
        "###);
    }

    {
        let (stdout, stderr) = git.run(&[
            "test",
            "run",
            "--strategy",
            "worktree",
            "-x",
            "echo hello",
            "-vv",
            "@",
        ])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Ran echo hello on 1 commit:
        ✓ Passed (cached): 62fc20d create test1.txt
        Stdout: <repo-path>/.git/branchless/test/8108c01b1930423879f106c1ebf725fcbfedccda/echo_hello/stdout
        hello
        Stderr: <repo-path>/.git/branchless/test/8108c01b1930423879f106c1ebf725fcbfedccda/echo_hello/stderr
        <no output>
        1 passed, 0 failed, 0 skipped
        hint: There was 1 cached test result.
        hint: To clear all cached results, run: git test clean
        hint: disable this hint by running: git config --global branchless.hint.cleanCachedTestResults false
        "###);
    }

    Ok(())
}

#[cfg(unix)] // Paths don't match on Windows.
#[test]
fn test_test_config_strategy() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;

    git.write_file(
        "test.sh",
        "#!/bin/sh
echo hello
",
    )?;
    git.commit_file("test2", 2)?;

    git.write_file_txt("test1", "Updated contents\n")?;

    git.run(&["config", "branchless.test.alias.default", "bash test.sh"])?;
    git.run(&["config", "branchless.test.strategy", "working-copy"])?;
    {
        let (stdout, stderr) = git.run_with_options(
            &["test", "run", "@"],
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

    git.run(&["config", "branchless.test.strategy", "worktree"])?;
    {
        let (stdout, stderr) = git.run(&["test", "run", "-vv", "@"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Ran bash test.sh on 1 commit:
        ✓ Passed: c82ebfa create test2.txt
        Stdout: <repo-path>/.git/branchless/test/a3ae41e24abf7537423d8c72d07df7af456de6dd/bash_test.sh/stdout
        hello
        Stderr: <repo-path>/.git/branchless/test/a3ae41e24abf7537423d8c72d07df7af456de6dd/bash_test.sh/stderr
        <no output>
        1 passed, 0 failed, 0 skipped
        "###);
    }

    git.run(&["config", "branchless.test.strategy", "invalid-value"])?;
    {
        let (stdout, stderr) = git.run_with_options(
            &["test", "run", "-vv", "@"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Invalid value for config value branchless.test.strategy: invalid-value
        Expected one of: working-copy, worktree
        "###);
    }

    Ok(())
}

#[test]
fn test_test_jobs_argument_handling() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.run(&["config", "branchless.test.alias.default", "exit 0"])?;

    {
        let (stdout, stderr) = git.run_with_options(
            &["test", "run", "--strategy", "working-copy", "--jobs", "0"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        The --jobs argument can only be used with --strategy worktree,
        but --strategy working-copy was provided instead.
        "###);
    }

    {
        // `--jobs 1` is allowed for `--strategy working-copy`, since that's the default anyways.
        let (stdout, _stderr) =
            git.run(&["test", "run", "--strategy", "working-copy", "--jobs", "1"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Ran exit 0 on 1 commit:
        ✓ Passed: 62fc20d create test1.txt
        1 passed, 0 failed, 0 skipped
        branchless: running command: <git-executable> rebase --abort
        "###);
    }

    {
        let (stdout, stderr) = git.run_with_options(
            &["test", "run", "--strategy", "working-copy", "--jobs", "2"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        The --jobs argument can only be used with --strategy worktree,
        but --strategy working-copy was provided instead.
        "###);
    }

    Ok(())
}
