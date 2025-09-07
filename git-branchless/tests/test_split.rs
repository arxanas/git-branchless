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
            o 01523cc temp(split): test2.txt (+1)
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
            o c4b067e temp(split): test2.txt (+1)
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
            o 5375cb6 temp(split): test1.txt (+1/-1)
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
            o de6e4df temp(split): test1.txt (-1)
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
            o 57020b0 temp(split): 2 files (+2)
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
            o 01523cc temp(split): test2.txt (+1)
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
            @ 01523cc (> branch-name) temp(split): test2.txt (+1)
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
            [1/1] Committed as: a629a22 create test3.txt
            branchless: processing 1 rewritten commit
            branchless: running command: <git-executable> checkout a629a22974b9232523701e66e6e2bcdf8ffc8ad1
            In-memory rebase succeeded.
            O f777ecc (master) create initial.txt
            |
            o 2932db7 first commit
            |
            o 01523cc temp(split): test2.txt (+1)
            |
            @ a629a22 create test3.txt
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
fn test_split_detach() -> eyre::Result<()> {
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
        let (stdout, _stderr) = git.branchless("split", &["HEAD~", "test2.txt", "--detach"])?;
        insta::assert_snapshot!(&stdout, @r###"
            Attempting rebase in-memory...
            [1/1] Committed as: f88fbe5 create test3.txt
            branchless: processing 1 rewritten commit
            branchless: running command: <git-executable> checkout f88fbe5901493ffe1c669cdb8aa5f056dc0bb605
            In-memory rebase succeeded.
            O f777ecc (master) create initial.txt
            |
            o 2932db7 first commit
            |\
            | o 01523cc temp(split): test2.txt (+1)
            |
            @ f88fbe5 create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test3.txt | 1 +
            2 files changed, 2 insertions(+)
        ");

        let (split_commit, _stderr) = git.run(&["query", "--raw", "exactly(siblings(HEAD), 1)"])?;
        let (stdout, _stderr) =
            git.run(&["show", "--pretty=format:", "--stat", split_commit.trim()])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 +
            1 file changed, 1 insertion(+)
        ");
    }

    Ok(())
}

#[test]
fn test_split_discard() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.write_file_txt("test3", "contents3")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    git.write_file_txt("test3", "updated contents3")?;
    git.write_file_txt("test4", "contents4")?;
    git.write_file_txt("test5", "contents5")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "second commit"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o e48cdc5 first commit
        |
        @ 8c3edf7 second commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test2.txt | 1 +
            test3.txt | 1 +
            3 files changed, 3 insertions(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test3.txt | 2 +-
            test4.txt | 1 +
            test5.txt | 1 +
            3 files changed, 3 insertions(+), 1 deletion(-)
        ");
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD~", "test2.txt", "--discard"])?;
        insta::assert_snapshot!(&stdout, @r###"
            Attempting rebase in-memory...
            [1/1] Committed as: 6e23d3d second commit
            branchless: processing 1 rewritten commit
            branchless: running command: <git-executable> checkout 6e23d3dfe1baeb366ebc31a61c32a19ca6a4ab63
            In-memory rebase succeeded.
            O f777ecc (master) create initial.txt
            |
            o 2932db7 first commit
            |
            @ 6e23d3d second commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test3.txt | 1 +
            2 files changed, 2 insertions(+)
        ");

        let (stdout, _stderr) = git.run(&["ls-files"])?;
        insta::assert_snapshot!(&stdout, @"
            initial.txt
            test1.txt
            test3.txt
            test4.txt
            test5.txt
        ");
    }

    {
        let (stdout, _stderr) =
            git.branchless("split", &["HEAD", "test3.txt", "test4.txt", "--discard"])?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: running command: <git-executable> checkout 6128a569e64c77d8a847293b81ae8c96357b751c
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            o 2932db7 first commit
            |
            @ 6128a56 second commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test5.txt | 1 +
            1 file changed, 1 insertion(+)
        ");

        let (stdout, _stderr) = git.run(&["ls-files"])?;
        insta::assert_snapshot!(&stdout, @"
            initial.txt
            test1.txt
            test3.txt
            test5.txt
        ");

        let (stdout, _stderr) = git.run(&["show", ":test3.txt"])?;
        insta::assert_snapshot!(&stdout, @"
            contents3
        ");
    }

    Ok(())
}

#[test]
fn test_split_discard_bug_checked_out_branch() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.write_file_txt("test3", "contents3")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    git.write_file_txt("test3", "updated contents3")?;
    git.write_file_txt("test4", "contents4")?;
    git.write_file_txt("test5", "contents5")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "second commit"])?;
    git.run(&["switch", "--create", "my-branch"])?;

    {
        // initial state:
        // HEAD~ contains new test1-3
        // HEAD contains update test3 and new test4&5
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o e48cdc5 first commit
        |
        @ 8c3edf7 (> my-branch) second commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test2.txt | 1 +
            test3.txt | 1 +
            3 files changed, 3 insertions(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test3.txt | 2 +-
            test4.txt | 1 +
            test5.txt | 1 +
            3 files changed, 3 insertions(+), 1 deletion(-)
        ");
    }

    {
        // discard test2 from HEAD~: should be removed from commit and disk

        let (stdout, _stderr) = git.branchless("split", &["HEAD~", "test2.txt", "--discard"])?;
        insta::assert_snapshot!(&stdout, @r###"
            Attempting rebase in-memory...
            [1/1] Committed as: 6e23d3d second commit
            branchless: processing 1 update: branch my-branch
            branchless: processing 1 rewritten commit
            branchless: running command: <git-executable> checkout my-branch
            In-memory rebase succeeded.
            O f777ecc (master) create initial.txt
            |
            o 2932db7 first commit
            |
            @ 6e23d3d (> my-branch) second commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test3.txt | 1 +
            2 files changed, 2 insertions(+)
        ");

        let (stdout, _stderr) = git.run(&["ls-files"])?;
        insta::assert_snapshot!(&stdout, @"
            initial.txt
            test1.txt
            test3.txt
            test4.txt
            test5.txt
        ");
    }

    {
        // discard test3 and test4 from HEAD: both should be removed from commit
        // but test3 should still exist on disk, with contents from HEAD~

        let (stdout, _stderr) =
            git.branchless("split", &["HEAD", "test3.txt", "test4.txt", "--discard"])?;
        insta::assert_snapshot!(&stdout, @r###"
            branchless: processing 1 update: branch my-branch
            Nothing to restack.
            O f777ecc (master) create initial.txt
            |
            o 2932db7 first commit
            |
            @ 6128a56 (> my-branch) second commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test5.txt | 1 +
            1 file changed, 1 insertion(+)
        ");

        let (stdout, _stderr) = git.run(&["ls-files"])?;
        insta::assert_snapshot!(&stdout, @"
            initial.txt
            test1.txt
            test3.txt
            test5.txt
        ");

        let (stdout, _stderr) = git.run(&["show", ":test3.txt"])?;
        insta::assert_snapshot!(&stdout, @"
            contents3
        ")
    }

    Ok(())
}

#[test]
fn test_split_insert_before_added_file() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    // new files
    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.write_file_txt("test3", "contents3")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    // modified file
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

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test2.txt | 1 +
            test3.txt | 1 +
            3 files changed, 3 insertions(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test3.txt | 2 +-
            1 file changed, 1 insertion(+), 1 deletion(-)
        ");
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD~", "test2.txt", "--before"])?;
        insta::assert_snapshot!(&stdout, @r###"
            Attempting rebase in-memory...
            [1/2] Committed as: 7014c04 first commit
            [2/2] Committed as: 22bd240 create test3.txt
            branchless: processing 2 rewritten commits
            branchless: running command: <git-executable> checkout 22bd2405a4660938b88615fb2b1283bfa2a52f8e
            In-memory rebase succeeded.
            O f777ecc (master) create initial.txt
            |
            o d02e8c5 temp(split): test2.txt (+1)
            |
            o 7014c04 first commit
            |
            @ 22bd240 create test3.txt
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~2"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 +
            1 file changed, 1 insertion(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test3.txt | 1 +
            2 files changed, 2 insertions(+)
        ");
    }

    Ok(())
}

#[test]
fn test_split_insert_before_modified_file() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    // new files
    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.write_file_txt("test3", "contents3")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    // modified files
    git.write_file_txt("test2", "contents2 again")?;
    git.write_file_txt("test3", "contents3 again")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "second commit"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o e48cdc5 first commit
        |
        @ 7249f22 second commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test2.txt | 1 +
            test3.txt | 1 +
            3 files changed, 3 insertions(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 2 +-
            test3.txt | 2 +-
            2 files changed, 2 insertions(+), 2 deletions(-)
        ");
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD", "test2.txt", "--before"])?;
        insta::assert_snapshot!(&stdout, @r###"
            Attempting rebase in-memory...
            [1/1] Committed as: 38fe8b7 second commit
            branchless: processing 1 rewritten commit
            branchless: running command: <git-executable> checkout 38fe8b76f889772efd0dd5cc1acb6ac02c85f9fb
            In-memory rebase succeeded.
            O f777ecc (master) create initial.txt
            |
            o e48cdc5 first commit
            |
            o 188b0a1 temp(split): test2.txt (+1/-1)
            |
            @ 38fe8b7 second commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 2 +-
            1 file changed, 1 insertion(+), 1 deletion(-)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test3.txt | 2 +-
            1 file changed, 1 insertion(+), 1 deletion(-)
        ");
    }

    Ok(())
}

#[test]
fn test_split_insert_before_deleted_file() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    // new files
    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.write_file_txt("test3", "contents3")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;

    // modified files
    git.delete_file("test2")?;
    git.write_file_txt("test3", "contents3 again")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "second commit"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o e48cdc5 first commit
        |
        @ 98ebe2f second commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test2.txt | 1 +
            test3.txt | 1 +
            3 files changed, 3 insertions(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 -
            test3.txt | 2 +-
            2 files changed, 1 insertion(+), 2 deletions(-)
        ");
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD", "test2.txt", "--before"])?;
        insta::assert_snapshot!(&stdout, @r###"
            Attempting rebase in-memory...
            [1/1] Committed as: f8502a2 second commit
            branchless: processing 1 rewritten commit
            branchless: running command: <git-executable> checkout f8502a26000b8f90597f6861d7f3c0330fdf4351
            In-memory rebase succeeded.
            O f777ecc (master) create initial.txt
            |
            o e48cdc5 first commit
            |
            o e5b771d temp(split): test2.txt (-1)
            |
            @ f8502a2 second commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 -
            1 file changed, 1 deletion(-)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test3.txt | 2 +-
            1 file changed, 1 insertion(+), 1 deletion(-)
        ");
    }

    Ok(())
}

#[test]
fn test_split_insert_before_attached_branch() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;
    git.run(&["switch", "-c", "branch-name"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 4d11d02 (> branch-name) first commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test2.txt | 1 +
            2 files changed, 2 insertions(+)
        ");
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD", "test2.txt", "--before"])?;
        insta::assert_snapshot!(&stdout, @r###"
            Attempting rebase in-memory...
            [1/1] Committed as: c678b65 first commit
            branchless: processing 1 update: branch branch-name
            branchless: processing 1 rewritten commit
            branchless: running command: <git-executable> checkout branch-name
            In-memory rebase succeeded.
            O f777ecc (master) create initial.txt
            |
            o d02e8c5 temp(split): test2.txt (+1)
            |
            @ c678b65 (> branch-name) first commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 +
            1 file changed, 1 insertion(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            1 file changed, 1 insertion(+)
        ");
    }

    Ok(())
}

#[test]
fn test_split_insert_before_detached_branch() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.detach_head()?;

    git.write_file_txt("test1", "contents1")?;
    git.write_file_txt("test2", "contents2")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "first commit"])?;
    git.run(&["branch", "branch-name"])?;

    {
        let (stdout, _stderr) = git.branchless("smartlog", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 4d11d02 (branch-name) first commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
            test2.txt | 1 +
            2 files changed, 2 insertions(+)
        ");
    }

    {
        let (stdout, _stderr) = git.branchless("split", &["HEAD", "test2.txt", "--before"])?;
        insta::assert_snapshot!(&stdout, @r###"
            Attempting rebase in-memory...
            [1/1] Committed as: c678b65 first commit
            branchless: processing 1 update: branch branch-name
            branchless: processing 1 rewritten commit
            branchless: running command: <git-executable> checkout c678b6529d8f33a6903e25f70327464bd77f1ca1
            In-memory rebase succeeded.
            O f777ecc (master) create initial.txt
            |
            o d02e8c5 temp(split): test2.txt (+1)
            |
            @ c678b65 (branch-name) first commit
        "###);

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD~"])?;
        insta::assert_snapshot!(&stdout, @"
            test2.txt | 1 +
            1 file changed, 1 insertion(+)
        ");

        let (stdout, _stderr) = git.run(&["show", "--pretty=format:", "--stat", "HEAD"])?;
        insta::assert_snapshot!(&stdout, @"
            test1.txt | 1 +
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
            [1/1] Committed as: a629a22 create test3.txt
            branchless: processing 1 rewritten commit
            branchless: running command: <git-executable> checkout a629a22974b9232523701e66e6e2bcdf8ffc8ad1
            In-memory rebase succeeded.
            O f777ecc (master) create initial.txt
            |
            o 2932db7 first commit
            |
            o 01523cc temp(split): test2.txt (+1)
            |
            @ a629a22 create test3.txt
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
            o 98da165 temp(split): subdir/test3.txt (+1)
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
            o 89564a0 temp(split): subdir/test1.txt (+1)
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
            o c3d37e6 temp(split): test2.txt (+1)
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
            o 9eeb11b temp(split): test1.txt (+1)
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
