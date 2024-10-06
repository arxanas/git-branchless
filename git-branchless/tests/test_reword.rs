use lib::testing::{make_git, GitRunOptions};

#[test]
fn test_reword_head() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.run(&["branch", "test1"])?;
    git.commit_file("test2", 2)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (test1) create test1.txt
    |
    @ 96d1c37 (> master) create test2.txt
    "###);

    git.branchless("reword", &["--force-rewrite", "--message", "foo"])?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (test1) create test1.txt
    |
    @ c1f5400 (> master) foo
    "###);

    Ok(())
}

#[test]
fn test_reword_current_commit_not_head() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.run(&["branch", "test1"])?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "test1"])?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    @ 62fc20d (> test1) create test1.txt
    |
    O 96d1c37 (master) create test2.txt
    "###);

    git.branchless("reword", &["--force-rewrite", "--message", "foo"])?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    @ a6f8868 (> test1) foo
    |
    O 5207ad5 (master) create test2.txt
    "###);

    Ok(())
}

#[test]
fn test_reword_with_multiple_messages() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    let (stdout, _stderr) = git.run(&["log", "-n", "1", "--format=%h%n%B"])?;
    insta::assert_snapshot!(stdout, @r###"
    62fc20d
    create test1.txt
    "###);

    git.branchless("reword", &["-f", "-m", "foo", "-m", "bar"])?;

    let (stdout, _stderr) = git.run(&["log", "-n", "1", "--format=%h%n%B"])?;
    insta::assert_snapshot!(stdout, @r###"
    34ae21e
    foo

    bar

    "###);

    Ok(())
}

#[test]
fn test_reword_preserves_comment_lines_for_messages_on_cli() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    let (stdout, _stderr) = git.run(&["log", "-n", "1", "--format=%h%n%B"])?;
    insta::assert_snapshot!(stdout, @r###"
    62fc20d
    create test1.txt
    "###);

    // try adding several messages that start w/ '#'
    git.branchless(
        "reword",
        &["-f", "-m", "foo", "-m", "# bar", "-m", "#", "-m", "buz"],
    )?;

    // confirm the '#' messages aren't present
    let (stdout, _stderr) = git.run(&["log", "-n", "1", "--format=%h%n%B"])?;
    insta::assert_snapshot!(stdout, @r###"
    11a0c54
    foo

    # bar

    #

    buz
    "###);

    Ok(())
}

#[test]
fn test_reword_non_head_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.run(&["branch", "test1"])?;
    git.commit_file("test2", 2)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (test1) create test1.txt
    |
    @ 96d1c37 (> master) create test2.txt
    "###);

    git.branchless("reword", &["HEAD^", "--force-rewrite", "--message", "bar"])?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 8d4a670 (test1) bar
    |
    @ 8f7f70e (> master) create test2.txt
    "###);

    Ok(())
}

#[test]
fn test_reword_multiple_commits_on_same_branch() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.run(&["branch", "test1"])?;
    git.commit_file("test2", 2)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (test1) create test1.txt
    |
    @ 96d1c37 (> master) create test2.txt
    "###);

    let (_stdout, _stderr) = git.branchless(
        "reword",
        &["HEAD", "HEAD^", "--force-rewrite", "--message", "foo"],
    )?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O a6f8868 (test1) foo
    |
    @ e2308b3 (> master) foo
    "###);

    Ok(())
}

#[test]
fn test_reword_tree() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.detach_head()?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;
    git.run(&["checkout", &test3_oid.to_string()])?;
    git.commit_file("test5", 5)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 96d1c37 (master) create test2.txt
    |
    o 70deb1e create test3.txt
    |\
    | o 355e173 create test4.txt
    |
    @ 9ea1b36 create test5.txt
    "###);

    let (_stdout, _stderr) =
        git.branchless("reword", &[&test3_oid.to_string(), "--message", "foo"])?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 96d1c37 (master) create test2.txt
    |
    o 929b68d foo
    |\
    | o a367935 create test4.txt
    |
    @ 38f9ce9 create test5.txt
    "###);

    Ok(())
}

#[test]
fn test_reword_across_branches() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
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

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |\
    | o 96d1c37 create test2.txt
    | |
    | o 70deb1e create test3.txt
    |
    o bf0d52a create test4.txt
    |
    @ 848121c create test5.txt
    "###);

    let (_stdout, _stderr) = git.branchless(
        "reword",
        &[
            &test2_oid.to_string(),
            &test4_oid.to_string(),
            "--message",
            "foo",
        ],
    )?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d (master) create test1.txt
    |\
    | o c1f5400 foo
    | |
    | o 1c9ad63 create test3.txt
    |
    o 3c442fc foo
    |
    @ 8648fbd create test5.txt
    "###);

    Ok(())
}

#[test]
fn test_reword_exit_early_public_commit() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;

    {
        let (stdout, _stderr) = git.branchless_with_options(
            "reword",
            &[],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        You are trying to rewrite 1 public commit, such as: 62fc20d create test1.txt
        It is generally not advised to rewrite public commits, because your
        collaborators will have difficulty merging your changes.
        Retry with -f/--force-rewrite to proceed anyways.
        "###);
    }

    Ok(())
}

#[test]
fn test_reword_fixup_head() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.run(&["checkout", "-b", "test"])?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    O f777ecc (master) create initial.txt
    |
    o 62fc20d create test1.txt
    |
    @ 96d1c37 (> test) create test2.txt
    "###);

    git.branchless("reword", &["--fixup", "HEAD^"])?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    O f777ecc (master) create initial.txt
    |
    o 62fc20d create test1.txt
    |
    @ 9a86e82 (> test) fixup! create test1.txt
    "###);

    Ok(())
}

#[test]
fn test_reword_fixup_non_head_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.run(&["checkout", "-b", "test"])?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    O f777ecc (master) create initial.txt
    |
    o 62fc20d create test1.txt
    |
    @ 96d1c37 (> test) create test2.txt
    "###);

    git.branchless("reword", &["HEAD^", "--fixup", "roots(all())"])?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    O f777ecc (master) create initial.txt
    |
    o aa7ed8f fixup! create initial.txt
    |
    @ 7e17323 (> test) create test2.txt
    "###);

    Ok(())
}

#[test]
fn test_reword_fixup_multiple_commits() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.run(&["checkout", "-b", "test"])?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    O f777ecc (master) create initial.txt
    |
    o 62fc20d create test1.txt
    |
    @ 96d1c37 (> test) create test2.txt
    "###);

    git.branchless("reword", &["stack()", "--fixup", "roots(all())"])?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    O f777ecc (master) create initial.txt
    |
    o aa7ed8f fixup! create initial.txt
    |
    @ 5a30497 (> test) fixup! create initial.txt
    "###);

    Ok(())
}

#[test]
fn test_reword_fixup_only_accepts_single_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.run(&["checkout", "-b", "test"])?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    O f777ecc (master) create initial.txt
    |
    o 62fc20d create test1.txt
    |
    @ 96d1c37 (> test) create test2.txt
    "###);

    let (_stdout, stderr) = git.branchless_with_options(
        "reword",
        &["--fixup", "ancestors(HEAD)"],
        &GitRunOptions {
            expected_exit_code: 1,
            ..Default::default()
        },
    )?;
    insta::assert_snapshot!(stderr, @r###"
        --fixup expects exactly 1 commit, but 'ancestors(HEAD)' evaluated to 3.
        Aborting.
    "###);

    Ok(())
}

#[test]
fn test_reword_fixup_ancestry_issue() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }
    git.init_repo()?;
    git.run(&["checkout", "-b", "test"])?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    let stdout = git.smartlog()?;
    insta::assert_snapshot!(stdout, @r###"
    O f777ecc (master) create initial.txt
    |
    o 62fc20d create test1.txt
    |
    @ 96d1c37 (> test) create test2.txt
    "###);

    let (_stdout, stderr) = git.branchless_with_options(
        "reword",
        &["HEAD^", "--fixup", "HEAD"],
        &GitRunOptions {
            expected_exit_code: 1,
            ..Default::default()
        },
    )?;
    insta::assert_snapshot!(stderr, @r###"
        The commit supplied to --fixup must be an ancestor of all commits being reworded.
        Aborting.
    "###);

    Ok(())
}

#[test]
fn test_reword_merge_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.detach_head()?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file("test3", 3)?;
    git.run(&["merge", &test2_oid.to_string()])?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o 96d1c37 create test2.txt
        | & (merge) a4dd9b0 Merge commit '96d1c37a3d4363611c49f7e52186e189a04c531f' into HEAD
        |
        o 4838e49 create test3.txt
        |
        | & (merge) 96d1c37 create test2.txt
        |/
        @ a4dd9b0 Merge commit '96d1c37a3d4363611c49f7e52186e189a04c531f' into HEAD
        "###);
    }

    {
        let (stdout, stderr) = git.branchless("reword", &["-m", "new message"])?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: creating working copy snapshot
        Previous HEAD position was a4dd9b0 Merge commit '96d1c37a3d4363611c49f7e52186e189a04c531f' into HEAD
        branchless: processing 1 update: ref HEAD
        HEAD is now at 2fc54bd new message
        branchless: processing checkout
        "###);
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 2fc54bd new message
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout 2fc54bd59c79078e6d9012df241bcc90f0199596
        In-memory rebase succeeded.
        Reworded commit a4dd9b0 as 2fc54bd new message
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o 96d1c37 create test2.txt
        | & (merge) 2fc54bd new message
        |
        o 4838e49 create test3.txt
        |
        | & (merge) 96d1c37 create test2.txt
        |/
        @ 2fc54bd new message
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["diff", "master"])?;
        insta::assert_snapshot!(stdout, @r###"
        diff --git a/test2.txt b/test2.txt
        new file mode 100644
        index 0000000..4e512d2
        --- /dev/null
        +++ b/test2.txt
        @@ -0,0 +1 @@
        +test2 contents
        diff --git a/test3.txt b/test3.txt
        new file mode 100644
        index 0000000..a474f4e
        --- /dev/null
        +++ b/test3.txt
        @@ -0,0 +1 @@
        +test3 contents
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("reword", &["draft()", "-m", "new message 2"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/3] Committed as: 11f31c5 new message 2
        [2/3] Committed as: 800dd6c new message 2
        [3/3] Committed as: 930244c new message 2
        branchless: processing 3 rewritten commits
        branchless: running command: <git-executable> checkout 930244cad08ebb6278b3b606c45a6848dcc5cc74
        In-memory rebase succeeded.
        Reworded commit 96d1c37 as 800dd6c new message 2
        Reworded commit 4838e49 as 11f31c5 new message 2
        Reworded commit 2fc54bd as 930244c new message 2
        Reworded 3 commits. If this was unintentional, run: git undo
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        O 62fc20d (master) create test1.txt
        |\
        | o 800dd6c new message 2
        | & (merge) 930244c new message 2
        |
        o 11f31c5 new message 2
        |
        | & (merge) 800dd6c new message 2
        |/
        @ 930244c new message 2
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["diff", "master"])?;
        insta::assert_snapshot!(stdout, @r###"
        diff --git a/test2.txt b/test2.txt
        new file mode 100644
        index 0000000..4e512d2
        --- /dev/null
        +++ b/test2.txt
        @@ -0,0 +1 @@
        +test2 contents
        diff --git a/test3.txt b/test3.txt
        new file mode 100644
        index 0000000..a474f4e
        --- /dev/null
        +++ b/test3.txt
        @@ -0,0 +1 @@
        +test3 contents
        "###);
    }

    Ok(())
}
