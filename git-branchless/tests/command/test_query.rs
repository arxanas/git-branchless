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
        70deb1e create test3.txt
        96d1c37 create test2.txt
        "###);
    }

    {
        let (stdout, stderr) = git.run(&["branchless", "query", ".^::", "--raw"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        70deb1e28791d8e7dd5a1f0c871a51b91282562f
        96d1c37a3d4363611c49f7e52186e189a04c531f
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
        Expected one of "(", ")", "..", ":", "::", a commit/branch/tag or a string literal
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
        insta::assert_snapshot!(stderr, @"Evaluation error for expression 'foo': no commit, branch, or reference with the name 'foo' could be found
");
        insta::assert_snapshot!(stdout, @"");
    }

    {
        let (stdout, stderr) = git.run_with_options(
            &["branchless", "query", "foo()"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @r###"
        Evaluation error for expression 'foo()': no function with the name 'foo' could be found; these functions are available: all, ancestors, author.email, author.name, branches, children, committer.email, committer.name, descendants, difference, draft, heads, intersection, message, none, not, nthancestor, nthparent, only, parents, paths.changed, range, roots, stack, union
        "###);
        insta::assert_snapshot!(stdout, @"");
    }

    Ok(())
}

#[test]
fn test_query_legacy_git_syntax() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    {
        let (stdout, stderr) = git.run(&["branchless", "query", "HEAD~2"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        62fc20d create test1.txt
        "###);
    }

    {
        let (stdout, stderr) = git.run_with_options(
            &["branchless", "query", "foo-@"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"Evaluation error for expression 'foo-@': no commit, branch, or reference with the name 'foo-@' could be found
");
        insta::assert_snapshot!(stdout, @"");
    }

    Ok(())
}

#[test]
fn test_query_branches() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.run(&["branch", "foo"])?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    {
        let (stdout, _stderr) = git.run(&["branchless", "query", "-b", "."])?;
        insta::assert_snapshot!(stdout, @"master
");
    }

    {
        let (stdout, _stderr) = git.run_with_options(
            &["branchless", "query", "-b", ".", "-r"],
            &GitRunOptions {
                expected_exit_code: 2,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @"");
    }

    {
        let (stdout, _stderr) = git.run(&["branchless", "query", "-b", "::."])?;
        insta::assert_snapshot!(stdout, @r###"
        master
        foo
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["branchless", "query", "branches()"])?;
        insta::assert_snapshot!(stdout, @r###"
        70deb1e create test3.txt
        62fc20d create test1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_query_hidden_commits() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    let test2_oid = git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    git.run(&["hide", "HEAD"])?;
    git.run(&["checkout", &test2_oid.to_string()])?;

    {
        let (stdout, stderr) = git.run(&["branchless", "query", &format!("{}::", test2_oid)])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        96d1c37 create test2.txt
        "###);
    }

    Ok(())
}
