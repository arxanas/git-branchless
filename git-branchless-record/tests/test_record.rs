use lib::testing::pty::{DOWN_ARROW, PtyAction, run_in_pty};
use lib::testing::{GitInitOptions, GitRunOptions, make_git};

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
fn test_record_with_new_untracked_files() -> eyre::Result<()> {
    //
    // This test mostly mimics the corresponding test for `amend`. Changes here
    // may also need to be made there.
    //
    // See fn test_amend_with_new_untracked_files in git-branchless/tests/test_amend.rs
    //

    let git = make_git()?;
    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    {
        // confirm initial state: only test1

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            1 file changed, 1 insertion(+)
        ");
    }

    {
        // working copy & disabled (default) => test2 not added
        git.write_file_txt("test1", "test1 updated 1")?;
        git.write_file_txt("test2", "test2 new")?;

        let (stdout, _stderr) = git.branchless("record", &["-m", "foo"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master 36be832] foo
         1 file changed, 1 insertion(+), 1 deletion(-)
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 2 +-
            1 file changed, 1 insertion(+), 1 deletion(-)
        ");
    }

    {
        // working copy & add
        git.write_file_txt("test1", "test1 updated 2")?;
        // test2 should still be considered "new" because last run was disabled

        let (stdout, _stderr) = git.branchless("record", &["-m", "foo", "--untracked", "add"])?;
        insta::assert_snapshot!(stdout, @r###"
        Including 1 new untracked file: test2.txt
        [master 765c01b] foo
         2 files changed, 2 insertions(+), 1 deletion(-)
         create mode 100644 test2.txt
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 2 +-
            test2.txt | 1 +
            2 files changed, 2 insertions(+), 1 deletion(-)
        ");
    }

    {
        // working copy & skip
        git.write_file_txt("test2", "test2 updated 3")?;
        git.write_file_txt("test3", "test3 new")?;

        let (stdout, _stderr) = git.branchless("record", &["-m", "foo", "--untracked", "skip"])?;
        insta::assert_snapshot!(stdout, @r###"
        Skipping 1 new untracked file: test3.txt
        hint: this file will remain skipped and will not be automatically reconsidered
        hint: to add it yourself: git add
        hint: disable this hint by running: git config --global branchless.hint.addSkippedFiles false
        [master 371fc94] foo
         1 file changed, 1 insertion(+), 1 deletion(-)
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 2 +-
            1 file changed, 1 insertion(+), 1 deletion(-)
        ");
    }

    {
        // working copy & prompt
        git.write_file_txt("test2", "test2 updated 4")?;
        // test3.txt should remain skipped because we've already seen it
        git.write_file_txt("test4", "test4 new")?;

        run_in_pty(
            &git,
            "record",
            &["-m", "foo", "--untracked", "prompt"],
            &[PtyAction::WaitUntilContains("test4"), PtyAction::Write("y")],
        )?;

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 2 +-
            test4.txt | 1 +
            2 files changed, 2 insertions(+), 1 deletion(-)
        ");
    }

    {
        // working copy & add
        // -> only new files in working copy, no changes to tracked files
        // test3.txt still skipped
        git.write_file_txt("test5", "test5 new")?;

        let (stdout, _stderr) = git.branchless("record", &["-m", "foo", "--untracked", "add"])?;
        insta::assert_snapshot!(stdout, @r###"
        Skipping 1 previously skipped file: test3.txt
        Including 1 new untracked file: test5.txt
        [master af69c1a] foo
         1 file changed, 1 insertion(+)
         create mode 100644 test5.txt
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test5.txt | 1 +
            1 file changed, 1 insertion(+)
        ");
    }

    {
        // index (staged) & add => untracked wc changes ignored
        git.write_file_txt("test5", "test5 updated 6")?;
        git.write_file_txt("test6", "test6 new")?;
        git.run(&["add", "test5.txt"])?;

        let (stdout, _stderr) = git.branchless("record", &["-m", "foo", "--untracked", "add"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master cbc1ddc] foo
         1 file changed, 1 insertion(+), 1 deletion(-)
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test5.txt | 2 +-
            1 file changed, 1 insertion(+), 1 deletion(-)
        ");
    }

    {
        // working copy & disable (again) => still no output about added/skipped files
        git.write_file_txt("test1", "test1 updated 7")?;

        let (stdout, _stderr) = git.branchless("record", &["-m", "foo"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master c3b6590] foo
         1 file changed, 1 insertion(+), 1 deletion(-)
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 2 +-
            1 file changed, 1 insertion(+), 1 deletion(-)
        ");
    }

    {
        // working copy & add (again)
        //  - test3 still skipped
        //  - test6 added because it was totally skipped during last/disabled run
        git.write_file_txt("test1", "test1 updated 8")?;

        let (stdout, _stderr) = git.branchless("record", &["-m", "foo", "--untracked", "add"])?;
        insta::assert_snapshot!(stdout, @r###"
        Skipping 1 previously skipped file: test3.txt
        Including 1 new untracked file: test6.txt
        [master be357c9] foo
         2 files changed, 2 insertions(+), 1 deletion(-)
         create mode 100644 test6.txt
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @r###"
        test1.txt | 2 +-
        test6.txt | 1 +
        2 files changed, 2 insertions(+), 1 deletion(-)
        "###);
    }

    Ok(())
}

#[test]
fn test_record_with_new_untracked_files_prompt() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    {
        // confirm initial state: only test1

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            1 file changed, 1 insertion(+)
        ");
    }

    {
        // working copy & prompt add
        git.write_file_txt("test2", "test2 new")?;

        run_in_pty(
            &git,
            "record",
            &["-m", "foo", "--untracked", "prompt"],
            &[PtyAction::WaitUntilContains("test2"), PtyAction::Write("y")],
        )?;

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 +
            1 file changed, 1 insertion(+)
        ");
    }

    {
        // working copy & prompt skip
        git.write_file_txt("test2", "test2 updated\nonce")?;
        git.write_file_txt("test3", "test3 new")?;

        run_in_pty(
            &git,
            "record",
            &["-m", "foo", "--untracked", "prompt"],
            &[PtyAction::WaitUntilContains("test3"), PtyAction::Write("n")],
        )?;

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 3 ++-
            1 file changed, 2 insertions(+), 1 deletion(-)
        ");
    }

    {
        // working copy & prompt skip remaining
        // - test3 should remain skipped,
        // - test4 should be added,
        // - test5 & 6 should both be skipped
        git.write_file_txt("test2", "test2 updated again")?;
        git.write_file_txt("test4", "test4 new")?;
        git.write_file_txt("test5", "test5 new")?;
        git.write_file_txt("test6", "test6 new")?;

        run_in_pty(
            &git,
            "record",
            &["-m", "foo", "--untracked", "prompt"],
            &[
                PtyAction::WaitUntilContains("test4"),
                PtyAction::Write("y"),
                PtyAction::WaitUntilContains("test5"),
                PtyAction::Write("o"),
            ],
        )?;

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @r###"
        test2.txt | 3 +--
        test4.txt | 1 +
        2 files changed, 2 insertions(+), 2 deletions(-)
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
fn test_record_new() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;

    {
        // --new w/ changes in the working copy
        git.write_file_txt("test1", "new test1 contents\n")?;

        let (stdout, _stderr) = git.branchless("record", &["-m", "empty commit 1", "--new"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout ebf97de456b71d33b95e6fd0a28139ece0f209d0 --
        M	test1.txt
        "###);

        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit ebf97de456b71d33b95e6fd0a28139ece0f209d0
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0100

            empty commit 1
        "###);

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        @ ebf97de empty commit 1
        "###);
    }

    {
        // --new w/ changes in the index
        git.run(&["add", "test1.txt"])?;

        let (stdout, _stderr) = git.branchless("record", &["-m", "empty commit 2", "--new"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 064c1bc41faa18fbe1332a5190976112f7b89fdb --
        M	test1.txt
        "###);

        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit 064c1bc41faa18fbe1332a5190976112f7b89fdb
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0100

            empty commit 2
        "###);

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |
        o ebf97de empty commit 1
        |
        @ 064c1bc empty commit 2
        "###);
    }

    Ok(())
}

#[test]
fn test_record_new_uses_user_name_and_email() -> eyre::Result<()> {
    let git = make_git()?;
    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    {
        let (stdout, _stderr) = git.run(&["show", "--name-only"])?;
        insta::assert_snapshot!(stdout, @r###"
            commit 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
            Author: Testy McTestface <test@example.com>
            Date:   Thu Oct 29 12:34:56 2020 -0100

                create test1.txt

            test1.txt
        "###);
    }

    {
        git.run(&["config", "user.name", "Uncreative User Name"])?;
        git.run(&["config", "user.email", "boring@example.com"])?;
        git.write_file_txt("test1", "new test1 contents\n")?;

        let (stdout, _stderr) = git.branchless_with_options(
            "record",
            &["-m", "empty commit 1", "--new"],
            &GitRunOptions {
                time: 2,
                env: {
                    [("TEST_RECORD_NEW_FAKE_COMMIT_TIME", "true")]
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect()
                },
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
            branchless: running command: <git-executable> checkout 30fa5a52eb8542e52686e6cd34bb40fdb99ba09f --
            M	test1.txt
        "###);

        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(_stderr+&stdout, @r###"
            commit 30fa5a52eb8542e52686e6cd34bb40fdb99ba09f
            Author: Uncreative User Name <boring@example.com>
            Date:   Thu Oct 29 08:34:56 2020 -0500

                empty commit 1
        "###);
    }

    Ok(())
}

#[test]
fn test_record_new_insert() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.run(&["switch", "--create", "test"])?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 1)?;
    git.run(&["checkout", "HEAD^"])?;
    git.write_file_txt("test1", "new test1 contents\n")?;

    let (stdout, _stderr) =
        git.branchless("record", &["-m", "empty commit", "--new", "--insert"])?;
    insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 910c8aacfa6e8f315cd578772e7bab2e636dacb5 --
        M	test1.txt
        Attempting rebase in-memory...
        [1/1] Committed as: ae12a93 create test2.txt
        branchless: processing 1 update: branch test
        branchless: processing 1 rewritten commit
        In-memory rebase succeeded.
    "###);

    let (stdout, _stderr) = git.run(&["show"])?;
    insta::assert_snapshot!(stdout, @r###"
        commit 910c8aacfa6e8f315cd578772e7bab2e636dacb5
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0100

            empty commit
    "###);

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 910c8aa empty commit
        |
        o ae12a93 (test) create test2.txt
    "###);

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
        branchless: running command: <git-executable> checkout master --
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
        branchless: running command: <git-executable> checkout master --
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
fn test_record_stash_detached_head() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.detach_head()?;

    {
        git.commit_file("test1", 1)?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 62fc20d create test1.txt
        "###);
    }

    {
        git.write_file_txt("test1", "new test1 contents\n")?;

        let (stdout, _stderr) = git.branchless("record", &["-m", "foo", "--stash"])?;
        insta::assert_snapshot!(stdout, @r###"
        [detached HEAD 9b6164c] foo
         1 file changed, 1 insertion(+), 1 deletion(-)
        branchless: running command: <git-executable> checkout 62fc20d2a290daea0d52bdc2ed2ad4be6491010e --
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 62fc20d create test1.txt
        |
        o 9b6164c foo
        "###);
    }

    Ok(())
}

#[test]
fn test_record_stash_default_message() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;

    {
        git.write_file_txt("test1", "new test1 contents\n")?;

        let (stdout, _stderr) = git.branchless("record", &["--stash"])?;
        insta::assert_snapshot!(stdout, @r###"
        [master fd2ffa4] stash: test1.txt (+1/-1)
         1 file changed, 1 insertion(+), 1 deletion(-)
        branchless: running command: <git-executable> branch -f master 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
        branchless: running command: <git-executable> checkout master --
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d (> master) create test1.txt
        |
        o fd2ffa4 stash: test1.txt (+1/-1)
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
        branchless: running command: <git-executable> checkout master -b foo --
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

#[test]
fn test_record_fixup() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.run(&["checkout", "-b", "test"])?;
    let test1_oid = git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.write_file_txt("test1", "update test1\n")?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r"
    O f777ecc (master) create initial.txt
    |
    o 62fc20d create test1.txt
    |
    @ 96d1c37 (> test) create test2.txt
    ");

    git.branchless("record", &["--fixup", "roots(all())"])?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r"
    O f777ecc (master) create initial.txt
    |
    o 62fc20d create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    @ 7b720ed (> test) fixup! create initial.txt
    ");

    git.run(&["checkout", &test1_oid.to_string()])?;
    git.write_file_txt("test1", "update test1 again\n")?;
    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r"
    O f777ecc (master) create initial.txt
    |
    @ 62fc20d create test1.txt
    |
    o 96d1c37 create test2.txt
    |
    o 7b720ed (test) fixup! create initial.txt
    ");

    let (_stdout, stderr) = git.branchless_with_options(
        "record",
        &["--fixup", &test2_oid.to_string()],
        &GitRunOptions {
            expected_exit_code: 1,
            ..Default::default()
        },
    )?;
    insta::assert_snapshot!(stderr, @r"
    The commit supplied to --fixup must be an ancestor of the commit being created.
    Aborting.
    ");

    Ok(())
}
