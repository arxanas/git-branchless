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

    git.write_file_txt("test2", "updated contents")?;

    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/2] Committed as: 7ac317b create test2.txt
        [2/2] Committed as: b51f01b create test3.txt
        branchless: processing 2 rewritten commits
        branchless: running command: <git-executable> reset 7ac317b9d1dd1bbdf46e8ee692b9b9e280f28a50
        branchless: running command: <git-executable> checkout 7ac317b9d1dd1bbdf46e8ee692b9b9e280f28a50
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 7ac317b create test2.txt
        |
        o b51f01b create test3.txt
        In-memory rebase succeeded.
        Amended with 1 uncommitted change.
        "###);
    }

    git.write_file_txt("test3", "create merge conflict")?;
    git.run(&["add", "."])?;
    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/2] Committed as: 7c5e857 create test2.txt
        This operation would cause a merge conflict, and --merge was not provided.
        Amending without rebasing descendants: 7ac317b create test2.txt
        branchless: running command: <git-executable> checkout 7c5e8578f402b6b77afa143283b65fcdc9614233
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | x 7ac317b (rewritten as 7c5e8578) create test2.txt
        | |
        | o b51f01b create test3.txt
        |
        @ 7c5e857 create test2.txt
        hint: there is 1 abandoned commit in your commit graph
        hint: to fix this, run: git restack
        hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
        Amended with 1 staged change.
        To resolve merge conflicts run: git restack --merge
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | x 7ac317b (rewritten as 7c5e8578) create test2.txt
        | |
        | o b51f01b create test3.txt
        |
        @ 7c5e857 create test2.txt
        hint: there is 1 abandoned commit in your commit graph
        hint: to fix this, run: git restack
        hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
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
        Attempting rebase in-memory...
        [1/1] Committed as: f6b2553 create test2.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> reset f6b255388219264f4bcd258a3020d262c2d7b03e
        branchless: running command: <git-executable> checkout f6b255388219264f4bcd258a3020d262c2d7b03e
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f6b2553 create test2.txt
        In-memory rebase succeeded.
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
        Attempting rebase in-memory...
        [1/1] Committed as: f0f0727 create test2.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> reset f0f07277a6448cac370e6023ab379ec0c601ccfe
        branchless: running command: <git-executable> checkout f0f07277a6448cac370e6023ab379ec0c601ccfe
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f0f0727 create test2.txt
        In-memory rebase succeeded.
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
        Attempting rebase in-memory...
        [1/1] Committed as: f0f0727 create test2.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> reset f0f07277a6448cac370e6023ab379ec0c601ccfe
        branchless: running command: <git-executable> checkout f0f07277a6448cac370e6023ab379ec0c601ccfe
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f0f0727 create test2.txt
        In-memory rebase succeeded.
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

    git.write_file_txt("test1", "updated contents")?;
    git.write_file_txt("test2", "updated contents")?;
    git.run(&["add", "test1.txt"])?;

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        HEAD detached from f777ecc
        Changes to be committed:
          (use "git restore --staged <file>..." to unstage)
        	modified:   test1.txt

        Changes not staged for commit:
          (use "git add <file>..." to update what will be committed)
          (use "git restore <file>..." to discard changes in working directory)
        	modified:   test2.txt

        Changes to be committed:
        diff --git c/test1.txt i/test1.txt
        index 7432a8f..53cd939 100644
        --- c/test1.txt
        +++ i/test1.txt
        @@ -1 +1 @@
        -test1 contents
        +updated contents
        \ No newline at end of file
        --------------------------------------------------
        Changes not staged for commit:
        diff --git i/test2.txt w/test2.txt
        index 4e512d2..53cd939 100644
        --- i/test2.txt
        +++ w/test2.txt
        @@ -1 +1 @@
        -test2 contents
        +updated contents
        \ No newline at end of file
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: f8e4ba1 create test2.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> reset f8e4ba1be5cefcf22e831f51b1525b0be8215a31
        Unstaged changes after reset:
        M	test2.txt
        branchless: running command: <git-executable> checkout f8e4ba1be5cefcf22e831f51b1525b0be8215a31
        M	test2.txt
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f8e4ba1 create test2.txt
        In-memory rebase succeeded.
        Amended with 1 staged change. (Some uncommitted changes were not amended.)
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        HEAD detached from f777ecc
        Changes not staged for commit:
          (use "git add <file>..." to update what will be committed)
          (use "git restore <file>..." to discard changes in working directory)
        	modified:   test2.txt

        --------------------------------------------------
        Changes not staged for commit:
        diff --git i/test2.txt w/test2.txt
        index 4e512d2..53cd939 100644
        --- i/test2.txt
        +++ w/test2.txt
        @@ -1 +1 @@
        -test2 contents
        +updated contents
        \ No newline at end of file
        no changes added to commit (use "git add" and/or "git commit -a")
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 2e69581 create test2.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> reset 2e69581cb466962fa85e5918f29af6d2925fdd6f
        branchless: running command: <git-executable> checkout 2e69581cb466962fa85e5918f29af6d2925fdd6f
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 2e69581 create test2.txt
        In-memory rebase succeeded.
        Amended with 1 uncommitted change.
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        HEAD detached from f777ecc
        nothing to commit, working tree clean
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
    git.write_file_txt("test1", "updated contents")?;

    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 3b98a96 create test1.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> reset 3b98a960e6ebde39a933c25413b43bce8c0fd128
        branchless: running command: <git-executable> checkout 3b98a960e6ebde39a933c25413b43bce8c0fd128
        O f777ecc (master) create initial.txt
        |
        @ 3b98a96 create test1.txt
        In-memory rebase succeeded.
        Amended with 1 uncommitted change.
        "###);
    }

    // Amend should only update tracked files.
    git.write_file_txt("newfile", "some new file")?;
    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @"There are no uncommitted or staged changes. Nothing to amend.
");
    }

    git.run(&["add", "."])?;
    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 685ef31 create test1.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> reset 685ef311b070a460b7c86a9aed068be563978021
        branchless: running command: <git-executable> checkout 685ef311b070a460b7c86a9aed068be563978021
        O f777ecc (master) create initial.txt
        |
        @ 685ef31 create test1.txt
        In-memory rebase succeeded.
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
    git.write_file_txt("executable_file", "contents")?;
    git.set_file_permissions("executable_file", executable)?;
    git.run(&["add", "."])?;

    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: f00ec4b create test2.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> reset f00ec4b5a81438f4e792ca5576a290b16fed8fdb
        branchless: running command: <git-executable> checkout f00ec4b5a81438f4e792ca5576a290b16fed8fdb
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f00ec4b create test2.txt
        In-memory rebase succeeded.
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
    git.write_file_txt("file1", "branch1 contents")?;
    git.run(&["commit", "-a", "-m", "updated"])?;
    git.run(&["checkout", "master"])?;
    git.write_file_txt("file1", "master contents")?;
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
    git.run(&["checkout", "-b", "foo"])?;

    git.commit_file("file1", 1)?;
    git.write_file_txt("file1", "new contents\n")?;

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        On branch foo
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
        Attempting rebase in-memory...
        [1/1] Committed as: 94b1077 create file1.txt
        branchless: processing 1 update: branch foo
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> reset foo
        branchless: running command: <git-executable> checkout foo
        O f777ecc (master) create initial.txt
        |
        @ 94b1077 (> foo) create file1.txt
        In-memory rebase succeeded.
        Amended with 1 uncommitted change.
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status"])?;
        insta::assert_snapshot!(stdout, @r###"
        On branch foo
        nothing to commit, working tree clean
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["undo", "-y"])?;
        insta::assert_snapshot!(stdout, @r###"
        Will apply these actions:
        1. Check out from 94b1077 create file1.txt
                       to 94b1077 create file1.txt
        2. Check out from 94b1077 create file1.txt
                       to c0bdfb5 create file1.txt
        3. Restore snapshot for c0bdfb5 create file1.txt
                backed up using 55e9304 branchless: automated working copy snapshot
        4. Rewrite commit 94b1077 create file1.txt
                      as c0bdfb5 create file1.txt
        5. Move branch foo from 94b1077 create file1.txt
                             to c0bdfb5 create file1.txt
        6. Restore snapshot for branch foo
                    pointing to c0bdfb5 create file1.txt
                backed up using a293e0b branchless: automated working copy snapshot
        branchless: running command: <git-executable> checkout a293e0b4502882ced673f83b6742539ee06cbc74 -B foo
        branchless: running command: <git-executable> reset --hard HEAD
        HEAD is now at a293e0b branchless: automated working copy snapshot
        branchless: running command: <git-executable> checkout 7b6d0f10f68cf5df3de91f062c565e45f1b28006
        branchless: running command: <git-executable> reset c0bdfb5ba33c02bba2aa451efe2f220f12232408
        Unstaged changes after reset:
        M	file1.txt
        branchless: running command: <git-executable> update-ref refs/heads/foo c0bdfb5ba33c02bba2aa451efe2f220f12232408
        branchless: running command: <git-executable> symbolic-ref HEAD refs/heads/foo
        O f777ecc (master) create initial.txt
        |
        @ c0bdfb5 (> foo) create file1.txt
        Applied 6 inverse events.
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        On branch foo
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
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ c0bdfb5 (> foo) create file1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_amend_undo_detached_head() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("file1", 1)?;
    git.write_file_txt("file1", "new contents\n")?;

    {
        let (stdout, _stderr) = git.run(&["amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        Attempting rebase in-memory...
        [1/1] Committed as: 94b1077 create file1.txt
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> reset 94b10776514a5a182d920265fc3c42f2147b1201
        branchless: running command: <git-executable> checkout 94b10776514a5a182d920265fc3c42f2147b1201
        O f777ecc (master) create initial.txt
        |
        @ 94b1077 create file1.txt
        In-memory rebase succeeded.
        Amended with 1 uncommitted change.
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["undo", "-y"])?;
        insta::assert_snapshot!(stdout, @r###"
        Will apply these actions:
        1. Check out from 94b1077 create file1.txt
                       to 94b1077 create file1.txt
        2. Check out from 94b1077 create file1.txt
                       to c0bdfb5 create file1.txt
        3. Restore snapshot for c0bdfb5 create file1.txt
                backed up using 55e9304 branchless: automated working copy snapshot
        4. Rewrite commit 94b1077 create file1.txt
                      as c0bdfb5 create file1.txt
        5. Restore snapshot for c0bdfb5 create file1.txt
                backed up using 55e9304 branchless: automated working copy snapshot
        branchless: running command: <git-executable> checkout 55e9304c975103af25622dca880679182506f49f
        branchless: running command: <git-executable> reset --hard HEAD
        HEAD is now at 55e9304 branchless: automated working copy snapshot
        branchless: running command: <git-executable> checkout 7b6d0f10f68cf5df3de91f062c565e45f1b28006
        branchless: running command: <git-executable> reset c0bdfb5ba33c02bba2aa451efe2f220f12232408
        Unstaged changes after reset:
        M	file1.txt
        O f777ecc (master) create initial.txt
        |
        @ c0bdfb5 create file1.txt
        Applied 5 inverse events.
        "###);
    }

    Ok(())
}
