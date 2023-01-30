        let (stdout, _stderr) = git.branchless("test", &["run", "-x", "exit 0"])?;
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Tested 2 commits with exit 0:
        let (stdout, _stderr) = git.branchless_with_options(
            "test",
            &["run", "-x", "exit 1"],
        Using test execution strategy: working-copy
        X Failed (exit code 1): fe65c1f create test2.txt
        X Failed (exit code 1): 0206717 create test3.txt
        Tested 2 commits with exit 1:
        0 passed, 2 failed, 0 skipped
        let (stdout, _stderr) = git.branchless_with_options(
            "test",
            &["run", "-x", "kill $PPID"],
        Using test execution strategy: working-copy
        let (stdout, _stderr) = git.branchless_with_options(
            "test",
            &["run", "-x", "exit 0"],
        let (stdout, _stderr) = git.branchless("test", &["run", "-x", "exit 0"])?;
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Tested 3 commits with exit 0:
        let (stdout, _stderr) = git.branchless("test", &["run", "-x", "exit 0"])?;
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Tested 3 commits with exit 0:
        let (stdout, _stderr) = git.branchless("test", &["run", "-x", short_command, "-v"])?;
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Tested 1 commit with bash test.sh 10:
        let (stdout, _stderr) = git.branchless("test", &["run", "-x", short_command, "-vv"])?;
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Tested 1 commit with bash test.sh 10:
        let (stdout, _stderr) = git.branchless("test", &["run", "-x", long_command, "-v"])?;
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Tested 1 commit with bash test.sh 15:
        let (stdout, _stderr) = git.branchless("test", &["run", "-x", long_command, "-vv"])?;
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Tested 1 commit with bash test.sh 15:
        let (stdout, _stderr) = git.branchless("test", &["run", "-x", "echo hi", "."])?;
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Tested 1 commit with echo hi:
        let (stdout, stderr) = git.branchless("test", &["show", "-x", "echo hi"])?;
        let (stdout, stderr) = git.branchless("test", &["clean"])?;
        let (stdout, stderr) = git.branchless("test", &["show", "-x", "echo hi"])?;
        let (stdout, _stderr) = git.branchless_with_options(
            "test",
            &["run"],
        let (stdout, _stderr) = git.branchless_with_options(
            "test",
            &["run"],
        let (stdout, _stderr) = git.branchless_with_options(
            "test",
            &["run", "-c", "nonexistent"],
        let (stdout, _stderr) = git.branchless("test", &["run"])?;
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Tested 1 commit with echo default:
        let (stdout, _stderr) = git.branchless("test", &["run", "-c", "foo"])?;
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Tested 1 commit with echo foo:
        let (stdout, _stderr) = git.branchless_with_options(
            "test",
            &["run", "-c", "foo bar baz"],
        let (stdout, stderr) = git.branchless(
            &[
                "run",
                "--strategy",
                "worktree",
                "-x",
                "echo hello",
                "-vv",
                "@",
            ],
        )?;
        Using test execution strategy: worktree
        Tested 1 commit with echo hello:
        let (stdout, stderr) = git.branchless(
            &[
                "run",
                "--strategy",
                "worktree",
                "-x",
                "echo hello",
                "-vv",
                "@",
            ],
        )?;
        Using test execution strategy: worktree
        Tested 1 commit with echo hello:
        let (stdout, stderr) = git.branchless_with_options(
            "test",
            &["run", "@"],
        let (stdout, stderr) = git.branchless("test", &["run", "-vv", "@"])?;
        Using test execution strategy: worktree
        Tested 1 commit with bash test.sh:
        let (stdout, stderr) = git.branchless_with_options(
            "test",
            &["run", "-vv", "@"],
        let (stdout, _stderr) = git.branchless(
            &["run", "--strategy", "working-copy", "--jobs", "1"],
        )?;
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        Tested 1 commit with exit 0:
    {
        let (stdout, stderr) = git.branchless("test", &["run", "--jobs", "2"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Using test execution strategy: worktree
        ✓ Passed (cached): 62fc20d create test1.txt
        Tested 1 commit with exit 0:
        1 passed, 0 failed, 0 skipped
        hint: there was 1 cached test result
        hint: to clear these cached results, run: git test clean "stack() | @"
        hint: disable this hint by running: git config --global branchless.hint.cleanCachedTestResults false
        "###);
    }

    Ok(())
}

#[test]
fn test_test_fix() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        @ 70deb1e create test3.txt
        "###);
    }

    git.write_file(
        "test.sh",
        r#"#!/bin/sh
for i in *.txt; do
    echo "Updated contents for file $i" >"$i"
done
"#,
    )?;
    {
        let (stdout, _stderr) = git.branchless("test", &["fix", "-x", "bash test.sh"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        ✓ Passed (fixed): 62fc20d create test1.txt
        ✓ Passed (fixed): 96d1c37 create test2.txt
        ✓ Passed (fixed): 70deb1e create test3.txt
        Tested 3 commits with bash test.sh:
        3 passed, 0 failed, 0 skipped
        Attempting rebase in-memory...
        [1/3] Committed as: 300cb54 create test1.txt
        [2/3] Committed as: 2ee3aea create test2.txt
        [3/3] Committed as: 6f48e0a create test3.txt
        branchless: processing 3 rewritten commits
        branchless: running command: <git-executable> checkout 6f48e0a628753731739619f27107c57f5d0cc1e0
        In-memory rebase succeeded.
        Fixed 3 commits with bash test.sh:
        62fc20d create test1.txt
        96d1c37 create test2.txt
        70deb1e create test3.txt
        "###);
    }

    let original_log_output = {
        let (stdout, _stderr) = git.run(&["log", "--patch"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit 6f48e0a628753731739619f27107c57f5d0cc1e0
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0300

            create test3.txt

        diff --git a/test3.txt b/test3.txt
        new file mode 100644
        index 0000000..95c32b2
        --- /dev/null
        +++ b/test3.txt
        @@ -0,0 +1 @@
        +Updated contents for file test3.txt

        commit 2ee3aea583d4da25fa7996b788f4f27cbf7b9fd8
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0200

            create test2.txt

        diff --git a/test2.txt b/test2.txt
        new file mode 100644
        index 0000000..dce8610
        --- /dev/null
        +++ b/test2.txt
        @@ -0,0 +1 @@
        +Updated contents for file test2.txt

        commit 300cb542f9b6474befd598bdbdd263d7d2b011a0
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0100

            create test1.txt

        diff --git a/initial.txt b/initial.txt
        index 63af228..a48ef19 100644
        --- a/initial.txt
        +++ b/initial.txt
        @@ -1 +1 @@
        -initial contents
        +Updated contents for file initial.txt
        diff --git a/test1.txt b/test1.txt
        new file mode 100644
        index 0000000..4d62cad
        --- /dev/null
        +++ b/test1.txt
        @@ -0,0 +1 @@
        +Updated contents for file test1.txt

        commit f777ecc9b0db5ed372b2615695191a8a17f79f24
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            create initial.txt

        diff --git a/initial.txt b/initial.txt
        new file mode 100644
        index 0000000..63af228
        --- /dev/null
        +++ b/initial.txt
        @@ -0,0 +1 @@
        +initial contents
        "###);
        stdout
    };

    let updated_smartlog = {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 300cb54 create test1.txt
        |
        o 2ee3aea create test2.txt
        |
        @ 6f48e0a create test3.txt
        "###);
        stdout
    };

    // No changes should be made after the first invocation of the script, since
    // it was idempotent.
    {
        let (stdout, _stderr) = git.branchless("test", &["fix", "-x", "bash test.sh"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        ✓ Passed: 300cb54 create test1.txt
        ✓ Passed: 2ee3aea create test2.txt
        ✓ Passed: 6f48e0a create test3.txt
        Tested 3 commits with bash test.sh:
        3 passed, 0 failed, 0 skipped
        No commits to fix.
        "###);
    }
    {
        let (stdout, _stderr) = git.run(&["log", "--patch"])?;
        assert_eq!(stdout, original_log_output);
    }

    {
        let stdout = git.smartlog()?;
        assert_eq!(stdout, updated_smartlog);
    }

    Ok(())
}

#[test]
fn test_test_fix_failure() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    git.write_file(
        "test.sh",
        r#"#!/bin/sh
for i in *.txt; do
    if [[ "$i" == test2* ]]; then
        echo "Failed on $i"
        exit 1
    fi
    echo "Updated contents for file $i" >"$i"
done
"#,
    )?;

    {
        let (stdout, _stderr) = git.branchless_with_options(
            "test",
            &["fix", "-x", "bash test.sh"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        ✓ Passed (fixed): 62fc20d create test1.txt
        X Failed (exit code 1): 96d1c37 create test2.txt
        X Failed (exit code 1): 70deb1e create test3.txt
        Tested 3 commits with bash test.sh:
        1 passed, 2 failed, 0 skipped
        "###);
    }
    Ok(())
}

#[test]
fn test_test_no_apply_descendants_as_patches() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;

    git.write_file_txt("test1", "This file would conflict if applied as a patch\n")?;
    git.write_file_txt("test2", "Updated contents for file test2.txt\n")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "descendant commit"])?;

    git.write_file(
        "test.sh",
        r#"#!/bin/sh
for i in *.txt; do
    echo "Updated contents for file $i" >"$i"
done
"#,
    )?;

    {
        let (stdout, _stderr) = git.run(&["log", "--patch"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit 75e728fb17f6952287302c3e76d88aa737dd99d1
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            descendant commit

        diff --git a/test1.txt b/test1.txt
        index 7432a8f..60d9cdb 100644
        --- a/test1.txt
        +++ b/test1.txt
        @@ -1 +1 @@
        -test1 contents
        +This file would conflict if applied as a patch
        diff --git a/test2.txt b/test2.txt
        new file mode 100644
        index 0000000..dce8610
        --- /dev/null
        +++ b/test2.txt
        @@ -0,0 +1 @@
        +Updated contents for file test2.txt

        commit 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0100

            create test1.txt

        diff --git a/test1.txt b/test1.txt
        new file mode 100644
        index 0000000..7432a8f
        --- /dev/null
        +++ b/test1.txt
        @@ -0,0 +1 @@
        +test1 contents

        commit f777ecc9b0db5ed372b2615695191a8a17f79f24
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            create initial.txt

        diff --git a/initial.txt b/initial.txt
        new file mode 100644
        index 0000000..63af228
        --- /dev/null
        +++ b/initial.txt
        @@ -0,0 +1 @@
        +initial contents
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("test", &["fix", "-x", "bash test.sh"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        Using test execution strategy: working-copy
        branchless: running command: <git-executable> rebase --abort
        ✓ Passed (fixed): 62fc20d create test1.txt
        ✓ Passed (fixed): 75e728f descendant commit
        Tested 2 commits with bash test.sh:
        2 passed, 0 failed, 0 skipped
        Attempting rebase in-memory...
        [1/2] Committed as: 300cb54 create test1.txt
        [2/2] Committed as: f15b423 descendant commit
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> checkout f15b423404bbebfe4b09e305e074b525d008f44a
        In-memory rebase succeeded.
        Fixed 2 commits with bash test.sh:
        62fc20d create test1.txt
        75e728f descendant commit
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["log", "--patch"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit f15b423404bbebfe4b09e305e074b525d008f44a
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            descendant commit

        diff --git a/test2.txt b/test2.txt
        new file mode 100644
        index 0000000..dce8610
        --- /dev/null
        +++ b/test2.txt
        @@ -0,0 +1 @@
        +Updated contents for file test2.txt

        commit 300cb542f9b6474befd598bdbdd263d7d2b011a0
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0100

            create test1.txt

        diff --git a/initial.txt b/initial.txt
        index 63af228..a48ef19 100644
        --- a/initial.txt
        +++ b/initial.txt
        @@ -1 +1 @@
        -initial contents
        +Updated contents for file initial.txt
        diff --git a/test1.txt b/test1.txt
        new file mode 100644
        index 0000000..4d62cad
        --- /dev/null
        +++ b/test1.txt
        @@ -0,0 +1 @@
        +Updated contents for file test1.txt

        commit f777ecc9b0db5ed372b2615695191a8a17f79f24
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            create initial.txt

        diff --git a/initial.txt b/initial.txt
        new file mode 100644
        index 0000000..63af228
        --- /dev/null
        +++ b/initial.txt
        @@ -0,0 +1 @@
        +initial contents
        "###);
    }
