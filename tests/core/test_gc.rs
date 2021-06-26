use branchless::testing::make_git;

#[test]
fn test_gc() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD^"])?;

    {
        let repo = git.get_repo()?;
        assert!(matches!(repo.revparse_single("62fc20d2"), Ok(_)));
    }

    git.run(&["gc", "--prune=now"])?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
@ f777ecc9 (master) create initial.txt
|
o 62fc20d2 create test1.txt
"###);
    }

    git.run(&["hide", "62fc20d2"])?;
    {
        let (stdout, _stderr) = git.run(&["branchless", "gc"])?;
        insta::assert_snapshot!(stdout, @r###"
branchless: collecting garbage
"###);
    }

    git.run(&["gc", "--prune=now"])?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
@ f777ecc9 (master) create initial.txt
"###);
    }

    {
        let repo = git.get_repo()?;
        assert!(matches!(repo.revparse_single("62fc20d2"), Err(_)));
    }

    Ok(())
}
