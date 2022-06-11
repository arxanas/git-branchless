use lib::testing::make_git;

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
