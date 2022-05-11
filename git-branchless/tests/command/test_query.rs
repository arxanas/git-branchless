use lib::testing::{make_git, GitRunOptions};

#[test]
fn test_query() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    {
        let (stdout, stderr) = git.run(&["branchless", "query", ".^::"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        96d1c37a3d4363611c49f7e52186e189a04c531f
        70deb1e28791d8e7dd5a1f0c871a51b91282562f
        "###);
    }

    Ok(())
}

#[test]
fn test_query_parse_error() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    {
        let (stdout, stderr) = git.run_with_options(
            &["branchless", "query", "foo("],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @r###"
        Parse error for expression 'foo(': parse error: Unrecognized EOF found at 4
        Expected one of "(", ")", "::", r#"[a-zA-Z0-9/_$@.-]+"# or r#"\\x22([^\\x22\\x5c]|\\x5c.)*\\x22"#
        "###);
        insta::assert_snapshot!(stdout, @"");
    }

    Ok(())
}

#[test]
fn test_query_eval_error() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    {
        let (stdout, stderr) = git.run_with_options(
            &["branchless", "query", "foo"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"Evaluation error for expression 'foo': name is not defined: 'foo'
");
        insta::assert_snapshot!(stdout, @"");
    }

    Ok(())
}
