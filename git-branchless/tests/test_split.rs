use std::path::PathBuf;

use lib::testing::{make_git, GitRunOptions};

#[test]
fn test_split_detached_head() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.write_file_txt("test3", "contents3")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ e48cdc5 first commit
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD", "test2.txt"])?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: running command: <git-executable> checkout 2932db7d1099237d79cbd43e29707d70e545d471
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            @ 2932db7 first commit
            |
            o c159d6a temp(split): test2.txt
        "###);
    }

    {
        git.branchless("next", &[])?;

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test3.txt | 1 +
            2 files changed, 2 insertions(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 +
            1 file changed, 1 insertion(+)
        ");
    }

    Ok(())
}

#[test]
fn test_split_added_file() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.commit_file("test1", 1)?;

    git.write_file_txt("test1", "updated contents")?;
    git.write_file_txt("test2", "new contents")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
            O f777ecc (master) create initial.txt
            |
            o 62fc20d create test1.txt
            |
            @ 0f6059d first commit
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD", "test2.txt"])?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: running command: <git-executable> checkout 2f9e232b389b1bc8035f4e5bde79f262c0af020c
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            o 62fc20d create test1.txt
            |
            @ 2f9e232 first commit
            |
            o 067feb9 temp(split): test2.txt
        "###);
    }

    {
        git.branchless("next", &[])?;

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 2 +-
            1 file changed, 1 insertion(+), 1 deletion(-)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 +
            1 file changed, 1 insertion(+)
        ");
    }

    Ok(())
}

#[test]
fn test_split_modified_file() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.commit_file("test1", 1)?;
    git.write_file_txt("test1", "updated contents")?;
    git.write_file_txt("test2", "new contents")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
            O f777ecc (master) create initial.txt
            |
            o 62fc20d create test1.txt
            |
            @ 0f6059d first commit
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD", "test1.txt"])?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: running command: <git-executable> checkout 495b4c09b4cc1755847ba0fd42c903f9c7eecc00
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            o 62fc20d create test1.txt
            |
            @ 495b4c0 first commit
            |
            o 590b05e temp(split): test1.txt
        "###);
    }

    {
        git.branchless("next", &[])?;

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 +
            1 file changed, 1 insertion(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 2 +-
            1 file changed, 1 insertion(+), 1 deletion(-)
        ");
    }

    Ok(())
}

#[test]
fn test_split_deleted_file() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.commit_file("test1", 1)?;

    git.delete_file("test1")?;
    git.write_file_txt("test2", "new contents")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
            O f777ecc (master) create initial.txt
            |
            o 62fc20d create test1.txt
            |
            @ 94e9c28 first commit
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 -
            test2.txt | 1 +
            2 files changed, 1 insertion(+), 1 deletion(-)
        ");
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD", "test1.txt"])?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: running command: <git-executable> checkout 495b4c09b4cc1755847ba0fd42c903f9c7eecc00
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            o 62fc20d create test1.txt
            |
            @ 495b4c0 first commit
            |
            o bfc063a temp(split): test1.txt
        "###);
    }

    {
        git.branchless("next", &[])?;

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 +
            1 file changed, 1 insertion(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 -
            1 file changed, 1 deletion(-)
        ");
    }

    Ok(())
}

#[test]
fn test_split_multiple_files() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.write_file_txt("test3", "contents3")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ e48cdc5 first commit
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD", "test2.txt", "test3.txt"])?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: running command: <git-executable> checkout 8e5c74b7a1f09fc7ee1754763c810e3f00fe9b05
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            @ 8e5c74b first commit
            |
            o 0b1f3c6 temp(split): 2 files
        "###);
    }

    {
        git.branchless("next", &[])?;

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            1 file changed, 1 insertion(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 +
            test3.txt | 1 +
            2 files changed, 2 insertions(+)
        ");
    }

    Ok(())
}

#[test]
fn test_split_detached_branch() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.write_file_txt("test3", "contents3")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;
    git.run(&["branch", "branch-name"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ e48cdc5 (branch-name) first commit
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD", "test2.txt"])?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: processing 1 update: branch branch-name
            branchless: running command: <git-executable> checkout 2932db7d1099237d79cbd43e29707d70e545d471
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            @ 2932db7 (branch-name) first commit
            |
            o c159d6a temp(split): test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_split_attached_branch() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.write_file_txt("test3", "contents3")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;
    git.run(&["switch", "-c", "branch-name"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ e48cdc5 (> branch-name) first commit
        "###);

        let (stdout, _stderr) = git.run(&["status"])?;
        insta::assert_snapshot!(&stdout, @"
            On branch branch-name
            nothing to commit, working tree clean
        ");
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD", "test2.txt"])?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: processing 1 update: branch branch-name
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            o 2932db7 first commit
            |
            @ c159d6a (> branch-name) temp(split): test2.txt
        "###);

        let (stdout, _stderr) = git.run(&["status", "--short"])?;
        insta::assert_snapshot!(&stdout, @r#""#);
    }

    Ok(())
}

#[test]
fn test_split_restacks_descendents() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.write_file_txt("test3", "contents3")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    git.commit_file("test3", 1)?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o e48cdc5 first commit
        |
        @ 3d220e0 create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD~", "test2.txt"])?;
        insta::assert_snapshot!(&stdout, @r###"
            Attempting rebase in-memory...
            [1/1] Committed as: 71d03a3 create test3.txt
            branchless: processing 1 rewritten commit
            branchless: running command: <git-executable> checkout 71d03a33c534eda4253fc8772a4c0d5e9515127c
            In-memory rebase succeeded.
            O f777ecc (master) create initial.txt
            |
            o 2932db7 first commit
            |
            o c159d6a temp(split): test2.txt
            |
            @ 71d03a3 create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~2"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test3.txt | 1 +
            2 files changed, 2 insertions(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 +
            1 file changed, 1 insertion(+)
        ");
    }

    Ok(())
}

#[test]
fn test_split_undo_works() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.write_file_txt("test3", "contents3")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    git.commit_file("test3", 1)?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o e48cdc5 first commit
        |
        @ 3d220e0 create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD~", "test2.txt"])?;
        insta::assert_snapshot!(&stdout, @r###"
            Attempting rebase in-memory...
            [1/1] Committed as: 71d03a3 create test3.txt
            branchless: processing 1 rewritten commit
            branchless: running command: <git-executable> checkout 71d03a33c534eda4253fc8772a4c0d5e9515127c
            In-memory rebase succeeded.
            O f777ecc (master) create initial.txt
            |
            o 2932db7 first commit
            |
            o c159d6a temp(split): test2.txt
            |
            @ 71d03a3 create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~2"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test3.txt | 1 +
            2 files changed, 2 insertions(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 +
            1 file changed, 1 insertion(+)
        ");
    }

    {
        let (_stdout, _stderr) = git.branchless("undo", &["--yes"])?;

        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
            O f777ecc (master) create initial.txt
            |
            o e48cdc5 first commit
            |
            @ 3d220e0 create test3.txt
            "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test2.txt | 1 +
            test3.txt | 1 +
            3 files changed, 3 insertions(+)
        ");
    }

    Ok(())
}

#[test]
fn test_split_supports_absolute_relative_and_repo_relative_paths() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "root contents1")?;
    git.write_file_txt("test2", "root contents2")?;
    git.write_file_txt("subdir/test1", "subdir contents1")?;
    git.write_file_txt("subdir/test3", "subdir contents3")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 2998051 first commit
        "###);
    }

    {
        // test3.txt only exists in subdir

        let (stdout, _stderr) = git.branchless_with_options(
            "split",
            &["HEAD", "test3.txt"],
            &GitRunOptions {
                subdir: Some(PathBuf::from("subdir")),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: running command: <git-executable> checkout d9d41a308e25a71884831c865c356da43cc5294e
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            @ d9d41a3 first commit
            |
            o fc76a91 temp(split): subdir/test3.txt
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            subdir/test1.txt | 1 +
            test1.txt        | 1 +
            test2.txt        | 1 +
            3 files changed, 3 insertions(+)
        ");
    }

    {
        // test1.txt exists in root and subdir; try to resolve relative to cwd

        git.branchless("undo", &["--yes"])?;

        let (stdout, _stderr) = git.branchless_with_options(
            "split",
            &["HEAD", "test1.txt"],
            &GitRunOptions {
                subdir: Some(PathBuf::from("subdir")),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: running command: <git-executable> checkout 0cb81546d386a2064603c05ce7dc9759591f5a93
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            @ 0cb8154 first commit
            |
            o 5d2c1d0 temp(split): subdir/test1.txt
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            subdir/test3.txt | 1 +
            test1.txt        | 1 +
            test2.txt        | 1 +
            3 files changed, 3 insertions(+)
        ");
    }

    {
        // test2.txt only exists in root; resolve it relative to root

        git.branchless("undo", &["--yes"])?;

        let (stdout, _stderr) = git.branchless_with_options(
            "split",
            &["HEAD", "test2.txt"],
            &GitRunOptions {
                subdir: Some(PathBuf::from("subdir")),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: running command: <git-executable> checkout 912204674dfda3ab5fe089dddd1c9bf17b3c2965
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            @ 9122046 first commit
            |
            o ba3abaf temp(split): test2.txt
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            subdir/test1.txt | 1 +
            subdir/test3.txt | 1 +
            test1.txt        | 1 +
            3 files changed, 3 insertions(+)
        ");
    }

    {
        // test1.txt exists in root and subdir; support : to resolve relative to root

        git.branchless("undo", &["--yes"])?;

        let (stdout, _stderr) = git.branchless_with_options(
            "split",
            &["HEAD", ":/test1.txt"],
            &GitRunOptions {
                subdir: Some(PathBuf::from("subdir")),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: running command: <git-executable> checkout 6d0cd9b8fb1938e50250f30427a0d4865b351f2f
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            @ 6d0cd9b first commit
            |
            o 2f03a38 temp(split): test1.txt
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            subdir/test1.txt | 1 +
            subdir/test3.txt | 1 +
            test2.txt        | 1 +
            3 files changed, 3 insertions(+)
        ");
    }

    Ok(())
}

#[test]
fn test_split_unchanged_file() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "contents1")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 8e5c74b first commit
        "###);
    }

    {
        let (_stdout, stderr) = git.branchless_with_options(
            "split",
            &["HEAD", "initial.txt"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(&stderr, @r###"
            Aborting: file 'initial.txt' was not changed in commit 8e5c74b.
        "###);
    }

    Ok(())
}

#[test]
fn test_split_will_not_split_to_empty_commit() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "contents1")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 8e5c74b first commit
        "###);
    }

    {
        let (_stdout, stderr) = git.branchless_with_options(
            "split",
            &["HEAD", "test1.txt"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(&stderr, @r###"
            Aborting: refusing to split all changes out of commit 8e5c74b.
        "###);
    }

    Ok(())
}

// TODO report of which files were split and which were not found
