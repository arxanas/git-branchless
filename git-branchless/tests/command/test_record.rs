use crate::util::{run_in_pty, PtyAction};
use lib::testing::{make_git, GitInitOptions, GitRunOptions};

#[test]
fn test_record_unstaged_changes() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.write_file("test1", "contents1\n")?;
    {
        let (stdout, _stderr) = git.run(&["record", "-m", "foo"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master 914812a] foo
         1 file changed, 1 insertion(+), 1 deletion(-)
        "###);
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
fn test_record_unstaged_changes_interactive() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.write_file("test1", "contents1\n")?;
    {
        run_in_pty(
            &git,
            &["record", "-i", "-m", "foo"],
            &[
                PtyAction::WaitUntilContains("contents1"),
                PtyAction::Write("q"),
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
            &["record", "-i", "-m", "foo"],
            &[
                PtyAction::WaitUntilContains("contents1"),
                PtyAction::Write(" "),
                PtyAction::WaitUntilContains("[X]"),
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
    git.write_file("test1", "new test1 contents\n")?;
    git.run(&["add", "test1.txt"])?;

    {
        let (stdout, _stderr) = git.run(&["record", "-m", "foo"])?;
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
    git.write_file("test1", "new test1 contents\n")?;
    git.run(&["add", "test1.txt"])?;

    {
        let (stdout, _stderr) = git.run_with_options(
            &["record", "-i", "-m", "foo"],
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
    git.run(&["branchless", "init", "--main-branch", "master"])?;

    git.write_file("test1", "new test1 contents\n")?;
    git.run(&["add", "test1.txt"])?;
    {
        let (stdout, _stderr) = git.run(&["record", "-m", "foo", "--detach"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master (root-commit) d41ddf7] foo
         1 file changed, 1 insertion(+)
         create mode 100644 test1.txt
        branchless: running command: <git-executable> update-ref -d refs/heads/master d41ddf7dd8a526054ce1ebbc739f613824cecfef
        "###);
    }

    git.commit_file("test1", 1)?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        o d41ddf7 foo

        @ 6118a39 (> master) create test1.txt
        "###);
    }

    git.write_file("test1", "new test1 contents\n")?;
    {
        let (stdout, _stderr) = git.run(&["record", "-m", "foo", "--detach"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master 2e9aec4] foo
         1 file changed, 1 insertion(+), 1 deletion(-)
        branchless: running command: <git-executable> branch -f master 6118a39b4dd4c986d17da2123d907ac17696cb85
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        o d41ddf7 foo

        O 6118a39 (master) create test1.txt
        |
        @ 2e9aec4 foo
        "###);
    }

    Ok(())
}
