use lib::testing::pty::{run_in_pty, PtyAction, DOWN_ARROW};
use lib::testing::{make_git, GitInitOptions, GitRunOptions};

#[test]
fn test_record_unstaged_changes() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.write_file_txt("test1", "contents1\n")?;
    {
        let (stdout, _stderr) = git.branchless("record", &["-m", "foo", "-m", "bar"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master 872eae1] foo
         1 file changed, 1 insertion(+), 1 deletion(-)
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit 872eae10daf1e94d0c346540f6d655027c60e7ae
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            foo

            bar

        diff --git a/test1.txt b/test1.txt
        index 7432a8f..a024003 100644
        --- a/test1.txt
        +++ b/test1.txt
        @@ -1 +1 @@
        -test1 contents
        +contents1
        "###);
    }

    Ok(())
}

#[test]
fn test_record_unstaged_changes_interactive() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.write_file_txt("test1", "contents1\n")?;
    {
        run_in_pty(
            &git,
            "record",
            &["-i", "-m", "initial message"],
            &[
                PtyAction::WaitUntilContains("initial message"),
                PtyAction::Write("f"), // expand files
                PtyAction::WaitUntilContains("contents1"),
                PtyAction::Write("q"),
                PtyAction::Write(" "), // confirm quit dialog
            ],
        )?;
    }

    {
        // The above should not have committed anything.
        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
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
        "###);
    }

    {
        run_in_pty(
            &git,
            "record",
            &["-i", "-m", "foo"],
            &[
                PtyAction::Write("f"), // expand files
                PtyAction::WaitUntilContains("contents1"),
                PtyAction::Write(" "),
                PtyAction::WaitUntilContains("(●)"),
                PtyAction::Write("c"),
            ],
        )?;
    }

    {
        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit 914812ae3220add483f11d851dc59f0b5dbdeaa0
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            foo

        diff --git a/test1.txt b/test1.txt
        index 7432a8f..a024003 100644
        --- a/test1.txt
        +++ b/test1.txt
        @@ -1 +1 @@
        -test1 contents
        +contents1
        "###);
    }

    Ok(())
}

#[test]
fn test_record_staged_changes() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.write_file_txt("test1", "new test1 contents\n")?;
    git.run(&["add", "test1.txt"])?;

    {
        let (stdout, _stderr) = git.branchless("record", &["-m", "foo"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master b437fb4] foo
         1 file changed, 1 insertion(+), 1 deletion(-)
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit b437fb44ab49995a1b59877960830841ff7a7a23
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            foo

        diff --git a/test1.txt b/test1.txt
        index 7432a8f..2121042 100644
        --- a/test1.txt
        +++ b/test1.txt
        @@ -1 +1 @@
        -test1 contents
        +new test1 contents
        "###);
    }

    Ok(())
}

#[test]
fn test_record_staged_changes_interactive() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.write_file_txt("test1", "new test1 contents\n")?;
    git.run(&["add", "test1.txt"])?;

    {
        let (stdout, _stderr) = git.branchless_with_options(
            "record",
            &["-i", "-m", "foo"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Cannot select changes interactively while there are already staged changes.
        Either commit or unstage your changes and try again. Aborting.
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit 96d1c37a3d4363611c49f7e52186e189a04c531f
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0200

            create test2.txt

        diff --git a/test2.txt b/test2.txt
        new file mode 100644
        index 0000000..4e512d2
        --- /dev/null
        +++ b/test2.txt
        @@ -0,0 +1 @@
        +test2 contents
        "###);
    }

    Ok(())
}

#[test]
fn test_record_detach() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo_with_options(&GitInitOptions {
        make_initial_commit: false,
        run_branchless_init: false,
    })?;
    git.branchless("init", &["--main-branch", "master"])?;

    git.write_file_txt("test1", "new test1 contents\n")?;
    git.run(&["add", "test1.txt"])?;
    {
        let (stdout, _stderr) = git.branchless("record", &["-m", "foo", "--detach"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master (root-commit) d41ddf7] foo
         1 file changed, 1 insertion(+)
         create mode 100644 test1.txt
        branchless: running command: <git-executable> update-ref -d refs/heads/master d41ddf7dd8a526054ce1ebbc739f613824cecfef
        "###);
    }

    git.commit_file("test1", 1)?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        o d41ddf7 foo

        @ 6118a39 (> master) create test1.txt
        "###);
    }

    git.write_file_txt("test1", "new test1 contents\n")?;
    {
        let (stdout, _stderr) = git.branchless("record", &["-m", "foo", "--detach"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master 2e9aec4] foo
         1 file changed, 1 insertion(+), 1 deletion(-)
        branchless: running command: <git-executable> branch -f master 6118a39b4dd4c986d17da2123d907ac17696cb85
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        o d41ddf7 foo

        O 6118a39 (master) create test1.txt
        |
        @ 2e9aec4 foo
        "###);
    }

    Ok(())
}

#[test]
fn test_record_stash() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.write_file_txt("test1", "new test1 contents\n")?;
    git.run(&["add", "test1.txt"])?;
    {
        let (stdout, _stderr) = git.branchless("record", &["-m", "foo", "--stash"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master 4fe46bd] foo
         1 file changed, 1 insertion(+)
         create mode 100644 test1.txt
        branchless: running command: <git-executable> branch -f master f777ecc9b0db5ed372b2615695191a8a17f79f24
        branchless: running command: <git-executable> checkout master
        "###);
    }

    git.commit_file("test1", 1)?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
        | o 4fe46bd foo
        |
        @ 62fc20d (> master) create test1.txt
        "###);
    }

    git.write_file_txt("test1", "new test1 contents\n")?;
    {
        let (stdout, _stderr) = git.branchless("record", &["-m", "foo", "--stash"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master 9b6164c] foo
         1 file changed, 1 insertion(+), 1 deletion(-)
        branchless: running command: <git-executable> branch -f master 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
        branchless: running command: <git-executable> checkout master
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc create initial.txt
        |\
        | o 4fe46bd foo
        |
        @ 62fc20d (> master) create test1.txt
        |
        o 9b6164c foo
        "###);
    }

    Ok(())
}

#[test]
fn test_record_create_branch() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.write_file_txt("test1", "new contents\n")?;
    {
        let (stdout, _stderr) = git.branchless("record", &["-c", "foo", "-m", "Update"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout master -b foo
        M	test1.txt
        [foo 836023f] Update
         1 file changed, 1 insertion(+), 1 deletion(-)
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        @ 836023f (> foo) Update
        "###);
    }

    Ok(())
}

#[test]
fn test_record_insert() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.run(&["checkout", "-B", "foo"])?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD^"])?;

    git.write_file_txt("test1", "new contents\n")?;
    {
        let (stdout, _stderr) =
            git.branchless("record", &["-m", "update test1.txt", "--insert"])?;
        insta::assert_snapshot!(stdout, @r###"
        [detached HEAD c17ec22] update test1.txt
         1 file changed, 1 insertion(+), 1 deletion(-)
        Attempting rebase in-memory...
        [1/1] Committed as: 734e7f6 create test2.txt
        branchless: processing 1 update: branch foo
        branchless: processing 1 rewritten commit
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
        @ c17ec22 update test1.txt
        |
        o 734e7f6 (foo) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_record_insert_rewrite_public_commit() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD^"])?;

    git.write_file_txt("test1", "new contents\n")?;
    {
        let (stdout, _stderr) =
            git.branchless("record", &["-m", "update test1.txt", "--insert"])?;
        insta::assert_snapshot!(stdout, @r###"
        [detached HEAD c17ec22] update test1.txt
         1 file changed, 1 insertion(+), 1 deletion(-)
        You are trying to rewrite 1 public commit, such as: 96d1c37 create test2.txt
        It is generally not advised to rewrite public commits, because your
        collaborators will have difficulty merging your changes.
        To proceed anyways, run: git move -f -s 'siblings(.)
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d create test1.txt
        |\
        | @ c17ec22 update test1.txt
        |
        O 96d1c37 (master) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_record_insert_merge_conflict() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.run(&["checkout", "-B", "foo"])?;
    git.commit_file("test1", 1)?;
    git.commit_file_with_contents("test1", 1, "new contents 1\n")?;
    git.run(&["checkout", "HEAD^"])?;

    git.write_file_txt("test1", "new contents 2\n")?;
    {
        let (stdout, _stderr) =
            git.branchless("record", &["-m", "update test1.txt", "--insert"])?;
        insta::assert_snapshot!(stdout, @r###"
        [detached HEAD c36bf7c] update test1.txt
         1 file changed, 1 insertion(+), 1 deletion(-)
        Attempting rebase in-memory...
        This operation would cause a merge conflict:
        - (1 conflicting file) ae32734 create test1.txt
        To resolve merge conflicts, run: git move -m -s 'siblings(.)'
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | @ c36bf7c update test1.txt
        |
        o ae32734 (foo) create test1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_record_insert_obsolete_siblings() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["commit", "--amend", "-m", "updated message"])?;
    git.run(&["checkout", &test1_oid.to_string()])?;

    git.write_file_txt("test3", "test3 contents\n")?;
    git.run(&["add", "."])?;
    {
        let (stdout, _stderr) = git.run(&["record", "-I", "-m", "new commit"])?;
        insta::assert_snapshot!(stdout, @r###"
        [detached HEAD 1b3a1aa] new commit
         1 file changed, 1 insertion(+)
         create mode 100644 test3.txt
        Attempting rebase in-memory...
        [1/1] Committed as: 9253fc4 updated message
        branchless: processing 1 rewritten commit
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
        @ 1b3a1aa new commit
        |
        o 9253fc4 updated message
        "###);
    }

    Ok(())
}

#[cfg(unix)] // file modes behave differently on Windows
#[test]
fn test_record_file_mode_change() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.run(&["update-index", "--chmod=+x", "test1.txt"])?;
    git.branchless("record", &["-m", "update file mode"])?;
    {
        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit a0568bf022a0211551dd8e4d3b9db40288633e47
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            update file mode

        diff --git a/test1.txt b/test1.txt
        old mode 100644
        new mode 100755
        "###);
    }

    // The file mode change was only to the index, so we don't need to revert the
    // file mode on disk. It will already be non-executable on disk.
    {
        let (stdout, _stderr) = git.run(&["status"])?;
        insta::assert_snapshot!(stdout, @r###"
        HEAD detached from f777ecc
        Changes not staged for commit:
          (use "git add <file>..." to update what will be committed)
          (use "git restore <file>..." to discard changes in working directory)
        	modified:   test1.txt

        no changes added to commit (use "git add" and/or "git commit -a")
        "###);
    }

    git.write_file_txt("test1", "new contents\n")?;
    let exit_status = run_in_pty(
        &git,
        "record",
        &["-i", "-m", "update contents only"],
        &[
            PtyAction::WaitUntilContains("test1.txt"),
            PtyAction::Write("f"),        // expand files
            PtyAction::Write(DOWN_ARROW), // move to file mode
            PtyAction::Write(DOWN_ARROW), // move to changed contents
            PtyAction::Write(" "),
            PtyAction::Write("c"),
        ],
    )?;
    assert!(exit_status.success());

    {
        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit 6d2873ff3352132d510a9f98a169105529b31974
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            update contents only

        diff --git a/test1.txt b/test1.txt
        index 7432a8f..014fd71 100755
        --- a/test1.txt
        +++ b/test1.txt
        @@ -1 +1 @@
        -test1 contents
        +new contents
        "###);
    }
    {
        let (stdout, _stderr) = git.run(&["status"])?;
        insta::assert_snapshot!(stdout, @r###"
        HEAD detached from f777ecc
        Changes not staged for commit:
          (use "git add <file>..." to update what will be committed)
          (use "git restore <file>..." to discard changes in working directory)
        	modified:   test1.txt

        no changes added to commit (use "git add" and/or "git commit -a")
        "###);
    }

    let exit_status = run_in_pty(
        &git,
        "record",
        &["-i", "-m", "revert file mode"],
        &[
            PtyAction::WaitUntilContains("test1.txt"),
            PtyAction::Write(" "),
            PtyAction::Write("c"),
        ],
    )?;
    assert!(exit_status.success());
    {
        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit dae4fe3e4c7defeba5605f9ae9b018dce8993a8a
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            revert file mode

        diff --git a/test1.txt b/test1.txt
        old mode 100755
        new mode 100644
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status"])?;
        insta::assert_snapshot!(stdout, @r###"
        HEAD detached from f777ecc
        nothing to commit, working tree clean
        "###);
    }

    Ok(())
}

#[test]
fn test_record_binary_contents() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.write_file("foo", "initial text contents\n")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "initial foo"])?;

    git.write_file("foo", "initial binary contents\0")?;
    {
        let exit_status = run_in_pty(
            &git,
            "record",
            &["-i", "-m", "update foo to binary"],
            &[
                PtyAction::WaitUntilContains("foo"),
                PtyAction::Write(" "),
                PtyAction::Write("c"),
            ],
        )?;
        assert!(exit_status.success());
    }

    {
        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit 7f10d49c57bba90e7dbeb59282de954bd0a53535
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            update foo to binary

        diff --git a/foo b/foo
        index 21317ba..c2575e5 100644
        Binary files a/foo and b/foo differ
        "###);
    }

    Ok(())
}

#[test]
fn test_record_interactive_commit_message_template() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.write_file("commit-template.txt", "This is a commit template!\n")?;
    git.run(&["config", "commit.template", "commit-template.txt"])?;

    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.write_file_txt("test1", "updated contents\n")?;
    {
        let exit_status = run_in_pty(
            &git,
            "record",
            &["-i"],
            &[
                PtyAction::WaitUntilContains("test1"),
                PtyAction::Write(" "),
                PtyAction::Write("e"),
                PtyAction::Write("c"),
            ],
        )?;
        assert!(exit_status.success());
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 4ab51ca (master) create test1.txt
        |
        @ 88e5a87 This is a commit template!
        "###);
    }

    Ok(())
}
