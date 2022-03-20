use branchless::testing::make_git;

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

    let (stdout, _stderr) = git.run(&["smartlog"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d2 (test1) create test1.txt
    |
    @ 96d1c37a (> master) create test2.txt
    "###);

    git.run(&["reword", "--message", "foo"])?;

    let (stdout, _stderr) = git.run(&["smartlog"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d2 (test1) create test1.txt
    |
    @ b38039bd (> master) foo
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

    git.run(&["reword", "-m", "foo", "-m", "bar"])?;

    let (stdout, _stderr) = git.run(&["log", "-n", "1", "--format=%h%n%B"])?;
    insta::assert_snapshot!(stdout, @r###"
    961064b
    foo

    bar
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

    let (stdout, _stderr) = git.run(&["smartlog"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d2 (test1) create test1.txt
    |
    @ 96d1c37a (> master) create test2.txt
    "###);

    git.run(&["reword", "HEAD^", "--message", "bar"])?;

    let (stdout, _stderr) = git.run(&["smartlog"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 09b42f44 (test1) bar
    |
    @ 8ab8b750 (> master) create test2.txt
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

    let (stdout, _stderr) = git.run(&["smartlog"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d2 (test1) create test1.txt
    |
    @ 96d1c37a (> master) create test2.txt
    "###);

    let (_stdout, _stderr) =
        git.run(&["reword", "HEAD", "HEAD^", "--message", "foo", "--force"])?;

    let (stdout, _stderr) = git.run(&["smartlog"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 5b4452da (test1) foo
    |
    @ 12217a2f (> master) foo
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

    let (stdout, _stderr) = git.run(&["smartlog"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 96d1c37a (master) create test2.txt
    |
    o 70deb1e2 create test3.txt
    |\
    | o 355e173b create test4.txt
    |
    @ 9ea1b368 create test5.txt
    "###);

    let (_stdout, _stderr) = git.run(&["reword", &test3_oid.to_string(), "--message", "foo"])?;

    let (stdout, _stderr) = git.run(&["smartlog"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 96d1c37a (master) create test2.txt
    |
    o 4de8ed1a foo
    |\
    | o 93c8ffd1 create test4.txt
    |
    @ 92c4833c create test5.txt
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

    let (stdout, _stderr) = git.run(&["smartlog"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d2 (master) create test1.txt
    |\
    | o 96d1c37a create test2.txt
    | |
    | o 70deb1e2 create test3.txt
    |
    o bf0d52a6 create test4.txt
    |
    @ 848121cb create test5.txt
    "###);

    let (_stdout, _stderr) = git.run(&[
        "reword",
        &test2_oid.to_string(),
        &test4_oid.to_string(),
        "--message",
        "foo",
        "--force",
    ])?;

    let (stdout, _stderr) = git.run(&["smartlog"])?;
    insta::assert_snapshot!(stdout, @r###"
    :
    O 62fc20d2 (master) create test1.txt
    |\
    | o b38039bd foo
    | |
    | o 0c46d151 create test3.txt
    |
    o b376ed9e foo
    |
    @ a9fc71df create test5.txt
    "###);

    Ok(())
}
