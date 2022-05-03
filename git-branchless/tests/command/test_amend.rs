use lib::testing::{make_git, GitRunOptions};

#[test]
fn test_amend_with_children() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.run(&["checkout", "HEAD^"])?;

    git.write_file("test2", "updated contents")?;

    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset
        Attempting rebase in-memory...
        [1/1] Committed as: b51f01b create test3.txt
        branchless: processing 1 rewritten commit
        In-memory rebase succeeded.
        Finished restacking commits.
        No abandoned branches to restack.
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 7ac317b create test2.txt
        |
        o b51f01b create test3.txt
        Amended with 1 uncommitted change.
        "###);
    }

    git.write_file("test3", "create merge conflict")?;
    git.run(&["add", "."])?;
    {
        let (stdout, _stderr) = git.run_with_options(
            &["branchless", "amend"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        This operation would cause a merge conflict:
        - (1 conflicting file) b51f01b create test3.txt
        To resolve merge conflicts, run: git restack --merge
        "###);
    }

    Ok(())
}

#[test]
fn test_amend_rename() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    git.run(&["mv", "test1.txt", "moved.txt"])?;
    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        No abandoned commits to restack.
        No abandoned branches to restack.
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f6b2553 create test2.txt
        Amended with 2 staged changes.
        "###);
    }
    {
        let (stdout, _stderr) = git.run(&["show", "--raw", "--oneline", "HEAD"])?;
        insta::assert_snapshot!(stdout, @r###"
        f6b2553 create test2.txt
        :100644 100644 7432a8f 7432a8f R100	test1.txt	moved.txt
        :000000 100644 0000000 4e512d2 A	test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_amend_delete() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    git.delete_file("test1")?;
    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset
        No abandoned commits to restack.
        No abandoned branches to restack.
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f0f0727 create test2.txt
        Amended with 1 uncommitted change.
        "###);
    }
    {
        let (stdout, _stderr) = git.run(&["show", "--raw", "--oneline", "HEAD"])?;
        insta::assert_snapshot!(stdout, @r###"
        f0f0727 create test2.txt
        :100644 000000 7432a8f 0000000 D	test1.txt
        :000000 100644 0000000 4e512d2 A	test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_amend_delete_only_in_index() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    git.run(&["rm", "--cached", "test1.txt"])?;
    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        No abandoned commits to restack.
        No abandoned branches to restack.
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f0f0727 create test2.txt
        Amended with 1 staged change.
        "###);
    }
    {
        let (stdout, _stderr) = git.run(&["show", "--raw", "--oneline", "HEAD"])?;
        insta::assert_snapshot!(stdout, @r###"
        f0f0727 create test2.txt
        :100644 000000 7432a8f 0000000 D	test1.txt
        :000000 100644 0000000 4e512d2 A	test2.txt
        "###);
    }
    {
        let (stdout, _stderr) = git.run(&["status", "--porcelain=2"])?;
        insta::assert_snapshot!(stdout, @"? test1.txt
");
    }

    Ok(())
}

#[test]
fn test_amend_with_working_copy() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    git.write_file("test1", "updated contents")?;
    git.write_file("test2", "updated contents")?;
    git.run(&["add", "test1.txt"])?;

    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        No abandoned commits to restack.
        No abandoned branches to restack.
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f8e4ba1 create test2.txt
        Amended with 1 staged change. (Some uncommitted changes were not amended.)
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset
        No abandoned commits to restack.
        No abandoned branches to restack.
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 2e69581 create test2.txt
        Amended with 1 uncommitted change.
        "###);
    }

    Ok(())
}

#[test]
fn test_amend_head() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.write_file("test1", "updated contents")?;

    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset
        No abandoned commits to restack.
        No abandoned branches to restack.
        O f777ecc (master) create initial.txt
        |
        @ 3b98a96 create test1.txt
        Amended with 1 uncommitted change.
        "###);
    }

    // Amend should only update tracked files.
    git.write_file("newfile", "some new file")?;
    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @"There are no uncommitted or staged changes. Nothing to amend.
");
    }

    git.run(&["add", "."])?;
    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        No abandoned commits to restack.
        No abandoned branches to restack.
        O f777ecc (master) create initial.txt
        |
        @ 685ef31 create test1.txt
        Amended with 1 staged change.
        "###);
    }

    Ok(())
}

#[test]
#[cfg(unix)]
fn test_amend_executable() -> eyre::Result<()> {
    use std::{fs, os::unix::prelude::PermissionsExt};

    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    let executable = fs::Permissions::from_mode(0o777);
    git.write_file("executable_file", "contents")?;
    git.set_file_permissions("executable_file", executable)?;
    git.run(&["add", "."])?;

    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        No abandoned commits to restack.
        No abandoned branches to restack.
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f00ec4b create test2.txt
        Amended with 1 staged change.
        "###);
    }
    {
        let (stdout, _stderr) = git.run(&["show", "--raw", "--oneline", "HEAD"])?;
        insta::assert_snapshot!(stdout, @r###"
        f00ec4b create test2.txt
        :000000 100755 0000000 0839b2e A	executable_file.txt
        :000000 100644 0000000 4e512d2 A	test2.txt
        "###);
    }

    Ok(())
}
#[test]
#[cfg(unix)]
fn test_amend_unresolved_merge_conflict() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("file1", 1)?;
    git.run(&["checkout", "-b", "branch1"])?;
    git.write_file("file1", "branch1 contents")?;
    git.run(&["commit", "-a", "-m", "updated"])?;
    git.run(&["checkout", "master"])?;
    git.write_file("file1", "master contents")?;
    git.run(&["commit", "-a", "-m", "updated"])?;
    git.run_with_options(
        &["merge", "branch1"],
        &GitRunOptions {
            expected_exit_code: 1,
            ..Default::default()
        },
    )?;

    {
        let (stdout, _stderr) = git.run_with_options(
            &["branchless", "amend"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @"Cannot amend, because there are unresolved merge conflicts. Resolve the merge conflicts and try again.
");
    }

    Ok(())
}

#[test]
fn test_amend_undo() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("file1", 1)?;
    git.write_file("file1", "new contents\n")?;

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        On branch master
        Changes not staged for commit:
          (use "git add <file>..." to update what will be committed)
          (use "git restore <file>..." to discard changes in working directory)
        	modified:   file1.txt

        --------------------------------------------------
        Changes not staged for commit:
        diff --git i/file1.txt w/file1.txt
        index 84d55c5..014fd71 100644
        --- i/file1.txt
        +++ w/file1.txt
        @@ -1 +1 @@
        -file1 contents
        +new contents
        no changes added to commit (use "git add" and/or "git commit -a")
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset
        No abandoned commits to restack.
        No abandoned branches to restack.
        :
        @ 94b1077 (> master) create file1.txt
        Amended with 1 uncommitted change.
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status"])?;
        insta::assert_snapshot!(stdout, @r###"
        On branch master
        nothing to commit, working tree clean
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["undo", "-y"])?;
        insta::assert_snapshot!(stdout, @r###"
        Will apply these actions:
        1. Move branch master from 94b1077 create file1.txt
                                to 94b1077 create file1.txt
        2. Check out from 94b1077 create file1.txt
                       to 94b1077 create file1.txt
        3. Rewrite commit 94b1077 create file1.txt
                      as c0bdfb5 create file1.txt
        4. Restore snapshot for branch master
                    pointing to c0bdfb5 create file1.txt
                backed up using 80541b9 branchless: automated working copy commit
        branchless: running command: <git-executable> checkout 80541b9359e6a5eae4dda625a279cbac68a61f93 -B master
        branchless: running command: <git-executable> reset --hard HEAD
        HEAD is now at 80541b9 branchless: automated working copy commit
        branchless: running command: <git-executable> checkout 2e64218453f1f35f651c7e385cb5969966530f64
        branchless: running command: <git-executable> reset c0bdfb5ba33c02bba2aa451efe2f220f12232408
        Unstaged changes after reset:
        M	file1.txt
        :
        @ c0bdfb5 create file1.txt
        |
        O 80541b9 (master) branchless: automated working copy commit
        Applied 4 inverse events.
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        HEAD detached from 2e64218
        Changes not staged for commit:
          (use "git add <file>..." to update what will be committed)
          (use "git restore <file>..." to discard changes in working directory)
        	modified:   file1.txt

        --------------------------------------------------
        Changes not staged for commit:
        diff --git i/file1.txt w/file1.txt
        index 84d55c5..014fd71 100644
        --- i/file1.txt
        +++ w/file1.txt
        @@ -1 +1 @@
        -file1 contents
        +new contents
        no changes added to commit (use "git add" and/or "git commit -a")
        "###);
    }

    Ok(())
}
