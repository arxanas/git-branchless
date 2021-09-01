use branchless::testing::make_git;

#[test]
fn test_commands() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test", 1)?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
:
@ 3df4b935 (master) create test.txt
"###);
    }

    {
        let (stdout, _stderr) = git.run(&["hide", "3df4b935"])?;
        insta::assert_snapshot!(stdout, @r###"
Hid commit: 3df4b935 create test.txt
To unhide this commit, run: git unhide 3df4b935
"###);
    }

    {
        let (stdout, _stderr) = git.run(&["unhide", "3df4b935"])?;
        insta::assert_snapshot!(stdout, @r###"
Unhid commit: 3df4b935 create test.txt
To hide this commit, run: git hide 3df4b935
"###);
    }

    {
        let (stdout, _stderr) = git.run(&["prev"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout HEAD^
        @ f777ecc9 create initial.txt
        |
        O 3df4b935 (master) create test.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["next"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 3df4b9355b3b072aa6c50c6249bf32e289b3a661
        :
        @ 3df4b935 (master) create test.txt
        "###);
    }

    Ok(())
}
