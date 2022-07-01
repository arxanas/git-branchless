use std::ffi::OsStr;

use lib::testing::make_git;

#[test]
fn test_repair_broken_commit() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    let test3_oid = git.commit_file("test3", 3)?;
    git.run(&["checkout", "HEAD^"])?;

    let repo = git.get_repo()?;
    repo.get_reference(OsStr::new(&format!("refs/branchless/{test3_oid}")))?
        .unwrap()
        .delete()?;
    git.run(&["gc", "--prune=now"])?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        o 70deb1e <garbage collected>

        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 96d1c37 create test2.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["branchless", "repair"])?;
        insta::assert_snapshot!(stdout, @"Found and repaired 1 broken commit.
");
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 96d1c37 create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_repair_broken_branch() -> eyre::Result<()> {
    todo!();
}
