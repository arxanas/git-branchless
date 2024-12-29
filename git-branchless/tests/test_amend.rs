use lib::testing::{make_git, remove_rebase_lines, trim_lines, GitRunOptions};

#[test]
fn test_amend_with_children() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
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
        let (stdout, stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: creating working copy snapshot
        branchless: processing 1 update: ref HEAD
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset 7ac317b9d1dd1bbdf46e8ee692b9b9e280f28a50
        Attempting rebase in-memory...
        [1/1] Committed as: b51f01b create test3.txt
        branchless: processing 1 rewritten commit
        In-memory rebase succeeded.
        Restacked 1 commit.
        Amended with 1 uncommitted change.
        "###);
    }

    git.write_file_txt("test3", "create merge conflict")?;
    git.run(&["add", "."])?;

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        HEAD detached from 96d1c37
        Changes to be committed:
          (use "git restore --staged <file>..." to unstage)
        	new file:   test3.txt

        Changes to be committed:
        diff --git c/test3.txt i/test3.txt
        new file mode 100644
        index 0000000..4706e16
        --- /dev/null
        +++ i/test3.txt
        @@ -0,0 +1 @@
        +create merge conflict
        \ No newline at end of file
        "###);
    }

    {
        let git = git.duplicate_repo()?;
        {
            let (stdout, _stderr) = git.branchless("amend", &[])?;
            insta::assert_snapshot!(stdout, @r###"
            branchless: running command: <git-executable> reset 7c5e8578f402b6b77afa143283b65fcdc9614233
            Attempting rebase in-memory...
            This operation would cause a merge conflict:
            - (1 conflicting file) b51f01b create test3.txt
            To resolve merge conflicts, run: git restack --merge
            Amending without restacking descendant commits: 7ac317b create test2.txt
            Amended with 1 staged change.
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["status", "-vv"])?;
            insta::assert_snapshot!(stdout, @r###"
            HEAD detached from 96d1c37
            nothing to commit, working tree clean
            "###);
        }

        {
            let stdout = git.smartlog()?;
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
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset f6b255388219264f4bcd258a3020d262c2d7b03e
        Amended with 2 staged changes.
        "###);
    }
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f6b2553 create test2.txt
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
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset f0f07277a6448cac370e6023ab379ec0c601ccfe
        Amended with 1 uncommitted change.
        "###);
    }
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f0f0727 create test2.txt
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
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset f0f07277a6448cac370e6023ab379ec0c601ccfe
        Amended with 1 staged change.
        "###);
    }
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f0f0727 create test2.txt
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
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset f8e4ba1be5cefcf22e831f51b1525b0be8215a31
        Unstaged changes after reset:
        M	test2.txt
        Amended with 1 staged change. (Some uncommitted changes were not amended.)
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f8e4ba1 create test2.txt
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
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset 2e69581cb466962fa85e5918f29af6d2925fdd6f
        Amended with 1 uncommitted change.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ 2e69581 create test2.txt
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
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset 3b98a960e6ebde39a933c25413b43bce8c0fd128
        Amended with 1 uncommitted change.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 3b98a96 create test1.txt
        "###);
    }

    // Amend should only update tracked files.
    git.write_file_txt("newfile", "some new file")?;
    {
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @"There are no uncommitted or staged changes. Nothing to amend.
");
    }

    git.run(&["add", "."])?;
    {
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset 685ef311b070a460b7c86a9aed068be563978021
        Amended with 1 staged change.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 685ef31 create test1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_amend_head_with_file_with_space() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_committer_date_is_author_date()? {
        return Ok(());
    }

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test file with space", 1)?;
    git.write_file_txt("test file with space", "updated contents")?;

    {
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset 5ccdda3c1e0dca9aa58634b37cfa9cac09e3975b
        Amended with 1 uncommitted change.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 5ccdda3 create test file with space.txt
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
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset f00ec4b5a81438f4e792ca5576a290b16fed8fdb
        Amended with 1 staged change.
        "###);
    }
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        @ f00ec4b create test2.txt
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
        let (stdout, _stderr) = git.branchless_with_options(
            "amend",
            &[],
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
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: processing 1 update: branch foo
        branchless: running command: <git-executable> reset foo
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
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 94b1077 (> foo) create file1.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("undo", &["-y"])?;
        let stdout = trim_lines(stdout);
        insta::with_settings!({filters => vec![
            // ignore no-op checkouts, which is present in e.g. git 2.43
            // but disappears in e.g. git 2.46
            (r"\n2\. Check out from 94b1077 create file1.txt\s*to 94b1077 create file1.txt\n", "\n"),
            // ignore different numbering of events
            (r"(?m)^[0-9]+\. ", "#. "),
            ("Applied ([0-9]+) inverse events.", "Applied # inverse events."),
        ]}, {
        insta::assert_snapshot!(stdout, @r###"
        Will apply these actions:
        #. Move branch foo from 94b1077 create file1.txt
                             to 94b1077 create file1.txt
        #. Restore snapshot for branch foo
                    pointing to 94b1077 create file1.txt
                backed up using b4371f8 branchless: automated working copy snapshot
        #. Move branch foo from 94b1077 create file1.txt
                             to c0bdfb5 create file1.txt
        #. Rewrite commit 94b1077 create file1.txt
                      as c0bdfb5 create file1.txt
        #. Restore snapshot for branch foo
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
        Applied # inverse events.
        "###);
        });
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
        let stdout = git.smartlog()?;
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
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset 94b10776514a5a182d920265fc3c42f2147b1201
        Amended with 1 uncommitted change.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 94b1077 create file1.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("undo", &["-y"])?;
        insta::assert_snapshot!(stdout, @r###"
        Will apply these actions:
        1. Check out from 94b1077 create file1.txt
                       to c0bdfb5 create file1.txt
        2. Restore snapshot for c0bdfb5 create file1.txt
                backed up using 55e9304 branchless: automated working copy snapshot
        3. Rewrite commit 94b1077 create file1.txt
                      as c0bdfb5 create file1.txt
        4. Restore snapshot for c0bdfb5 create file1.txt
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
        Applied 4 inverse events.
        "###);
    }

    Ok(())
}

#[test]
fn test_amend_reparent() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD~"])?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        "###);
    }

    git.write_file_txt("test2", "Conflicting contents\n")?;
    git.run(&["add", "test2.txt"])?;
    {
        let (stdout, _stderr) = git.branchless(
            "amend",
            &[
                "--reparent",
                "--debug-dump-rebase-constraints",
                "--debug-dump-rebase-plan",
            ],
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset 3d8543b87d55c5b7995935e18e05cb6c399fb526
        Rebase constraints before adding descendants: [
            (
                NonZeroOid(3d8543b87d55c5b7995935e18e05cb6c399fb526),
                [
                    NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                ],
            ),
        ]
        Rebase constraints after adding descendants: [
            (
                NonZeroOid(3d8543b87d55c5b7995935e18e05cb6c399fb526),
                [
                    NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                ],
            ),
        ]
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: NonZeroOid(3d8543b87d55c5b7995935e18e05cb6c399fb526),
                commands: [
                    Reset {
                        target: Oid(
                            NonZeroOid(3d8543b87d55c5b7995935e18e05cb6c399fb526),
                        ),
                    },
                    Replace {
                        commit_oid: NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f),
                        replacement_commit_oid: NonZeroOid(e0d53059b0c1f7fa277f8206a3ec45b93d82ad4f),
                        parents: [
                            Oid(
                                NonZeroOid(3d8543b87d55c5b7995935e18e05cb6c399fb526),
                            ),
                        ],
                    },
                    RegisterExtraPostRewriteHook,
                ],
            },
        )
        Attempting rebase in-memory...
        [1/1] Committed as: e0d5305 create test2.txt
        branchless: processing 1 rewritten commit
        In-memory rebase succeeded.
        Restacked 1 commit.
        Amended with 1 staged change.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 3d8543b create test1.txt
        |
        o e0d5305 create test2.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit 3d8543b87d55c5b7995935e18e05cb6c399fb526
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0100

            create test1.txt

        diff --git a/test1.txt b/test1.txt
        new file mode 100644
        index 0000000..7432a8f
        --- /dev/null
        +++ b/test1.txt
        @@ -0,0 +1 @@
        +test1 contents
        diff --git a/test2.txt b/test2.txt
        new file mode 100644
        index 0000000..9e172d3
        --- /dev/null
        +++ b/test2.txt
        @@ -0,0 +1 @@
        +Conflicting contents
        "###);
    }

    git.branchless("next", &[])?;
    {
        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit e0d53059b0c1f7fa277f8206a3ec45b93d82ad4f
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0200

            create test2.txt

        diff --git a/test2.txt b/test2.txt
        index 9e172d3..4e512d2 100644
        --- a/test2.txt
        +++ b/test2.txt
        @@ -1 +1 @@
        -Conflicting contents
        +test2 contents
        "###);
    }

    Ok(())
}

#[test]
fn test_amend_reparent_merge() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    let conflicting_oid = git.commit_file_with_contents("conflicting", 2, "contents 1\n")?;
    let test3_oid = git.commit_file("test3", 3)?;

    git.run(&["checkout", "master", "--detach"])?;
    git.commit_file_with_contents("conflicting", 2, "contents 2\n")?;
    git.run(&["merge", &test3_oid.to_string(), "--strategy-option=ours"])?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | o 62fc20d create test1.txt
        | |
        | o 292c9a0 create conflicting.txt
        | |
        | o 402c2e6 create test3.txt
        | & (merge) 90403aa Merge commit '402c2e6c16e2861d57d7fb6a20cbc5559bd00d44' into HEAD
        |
        o 7ec39c7 create conflicting.txt
        |
        | & (merge) 402c2e6 create test3.txt
        |/
        @ 90403aa Merge commit '402c2e6c16e2861d57d7fb6a20cbc5559bd00d44' into HEAD
        "###);
    }
    {
        let (stdout, _stderr) = git.run(&["diff", "HEAD^^"])?;
        insta::assert_snapshot!(stdout, @r###"
        diff --git a/conflicting.txt b/conflicting.txt
        new file mode 100644
        index 0000000..076e8e3
        --- /dev/null
        +++ b/conflicting.txt
        @@ -0,0 +1 @@
        +contents 2
        diff --git a/test1.txt b/test1.txt
        new file mode 100644
        index 0000000..7432a8f
        --- /dev/null
        +++ b/test1.txt
        @@ -0,0 +1 @@
        +test1 contents
        diff --git a/test3.txt b/test3.txt
        new file mode 100644
        index 0000000..a474f4e
        --- /dev/null
        +++ b/test3.txt
        @@ -0,0 +1 @@
        +test3 contents
        "###);
    }

    git.run(&["checkout", &conflicting_oid.to_string()])?;
    git.write_file_txt("conflicting", "contents 3\n")?;
    {
        let (stdout, _stderr) =
            git.branchless("amend", &["--reparent", "--debug-dump-rebase-plan"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset d517a648915434edf38114da4efd820ec6f513cf
        Rebase plan: Some(
            RebasePlan {
                first_dest_oid: NonZeroOid(7ec39c7da50fc25deeea3318d937e1005de2a047),
                commands: [
                    Reset {
                        target: Oid(
                            NonZeroOid(7ec39c7da50fc25deeea3318d937e1005de2a047),
                        ),
                    },
                    Reset {
                        target: Oid(
                            NonZeroOid(d517a648915434edf38114da4efd820ec6f513cf),
                        ),
                    },
                    Replace {
                        commit_oid: NonZeroOid(402c2e6c16e2861d57d7fb6a20cbc5559bd00d44),
                        replacement_commit_oid: NonZeroOid(99a19d2c0c7b0fd225832483aa7fc3bbe37b44f5),
                        parents: [
                            Oid(
                                NonZeroOid(d517a648915434edf38114da4efd820ec6f513cf),
                            ),
                        ],
                    },
                    CreateLabel {
                        label_name: "parent-3",
                    },
                    Reset {
                        target: Oid(
                            NonZeroOid(7ec39c7da50fc25deeea3318d937e1005de2a047),
                        ),
                    },
                    Replace {
                        commit_oid: NonZeroOid(90403aa2d53c65bce0805026a2bb4a0934587bbe),
                        replacement_commit_oid: NonZeroOid(90403aa2d53c65bce0805026a2bb4a0934587bbe),
                        parents: [
                            Oid(
                                NonZeroOid(7ec39c7da50fc25deeea3318d937e1005de2a047),
                            ),
                            Label(
                                "parent-3",
                            ),
                        ],
                    },
                    RegisterExtraPostRewriteHook,
                ],
            },
        )
        Attempting rebase in-memory...
        [1/2] Committed as: 99a19d2 create test3.txt
        [2/2] Committed as: f66b478 Merge commit '402c2e6c16e2861d57d7fb6a20cbc5559bd00d44' into HEAD
        branchless: processing 2 rewritten commits
        In-memory rebase succeeded.
        Restacked 2 commits.
        Amended with 1 uncommitted change.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | o 62fc20d create test1.txt
        | |
        | @ d517a64 create conflicting.txt
        | |
        | o 99a19d2 create test3.txt
        | & (merge) f66b478 Merge commit '402c2e6c16e2861d57d7fb6a20cbc5559bd00d44' into HEAD
        |
        o 7ec39c7 create conflicting.txt
        |
        | & (merge) 99a19d2 create test3.txt
        |/
        o f66b478 Merge commit '402c2e6c16e2861d57d7fb6a20cbc5559bd00d44' into HEAD
        "###);
    }
    {
        let (stdout, _stderr) = git.run(&["diff", "master"])?;
        insta::assert_snapshot!(stdout, @r###"
        diff --git a/conflicting.txt b/conflicting.txt
        new file mode 100644
        index 0000000..e43ab8e
        --- /dev/null
        +++ b/conflicting.txt
        @@ -0,0 +1 @@
        +contents 3
        diff --git a/test1.txt b/test1.txt
        new file mode 100644
        index 0000000..7432a8f
        --- /dev/null
        +++ b/test1.txt
        @@ -0,0 +1 @@
        +test1 contents
        "###);
    }

    Ok(())
}

#[test]
fn test_amend_no_detach_branch() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.run(&["checkout", "-b", "foo"])?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "-b", "bar"])?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "foo"])?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 62fc20d (> foo) create test1.txt
        |
        o 96d1c37 (bar) create test2.txt
        "###);
    }

    git.write_file_txt("test1", "new contents\n")?;

    {
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: processing 1 update: branch foo
        branchless: running command: <git-executable> reset foo
        Attempting rebase in-memory...
        [1/1] Committed as: fc597fa create test2.txt
        branchless: processing 1 update: branch bar
        branchless: processing 1 rewritten commit
        branchless: running command: <git-executable> checkout foo
        In-memory rebase succeeded.
        Restacked 1 commit.
        Amended with 1 uncommitted change.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 7143ebc (> foo) create test1.txt
        |
        o fc597fa (bar) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_amend_merge() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD^"])?;

    git.write_file_txt("test2", "Conflicting contents\n")?;
    git.run(&["add", "."])?;

    {
        let (stdout, _stderr) = git.branchless_with_options(
            "amend",
            &["--merge"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        let stdout = remove_rebase_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset 3d8543b87d55c5b7995935e18e05cb6c399fb526
        Attempting rebase in-memory...
        Failed to merge in-memory, trying again on-disk...
        branchless: running command: <git-executable> diff --quiet
        Calling Git for on-disk rebase...
        branchless: running command: <git-executable> rebase --continue
        CONFLICT (add/add): Merge conflict in test2.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | @ 3d8543b create test1.txt
        |
        x 62fc20d (rewritten as 3d8543b8) create test1.txt
        |
        o 96d1c37 create test2.txt
        hint: there is 1 abandoned commit in your commit graph
        hint: to fix this, run: git restack
        hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
        "###);
    }

    git.write_file_txt("test2", "Resolved contents\n")?;
    git.run(&["add", "."])?;
    git.run(&["rebase", "--continue"])?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 3d8543b create test1.txt
        |
        o 47a1f4a create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_amend_move_detached_branch() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.run(&["branch", "foo"])?;

    git.write_file_txt("test1", "new contents\n")?;
    {
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: processing 1 update: branch foo
        branchless: running command: <git-executable> reset 7143ebcc44407b0553d9f50eaf29e0e4f0f0d6c0
        Amended with 1 uncommitted change.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        @ 7143ebc (foo) create test1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_amend_merge_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;
    git.detach_head()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file("test2", 2)?;
    git.run(&["merge", &test1_oid.to_string()])?;

    git.write_file_txt("test1", "new test1 contents\n")?;
    {
        let (stdout, _stderr) = git.branchless("amend", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset 3ebbc8fdaff7b5d5d0f1101feb3640d06b0297a2
        Amended with 1 uncommitted change.
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | o 62fc20d create test1.txt
        | & (merge) 3ebbc8f Merge commit '62fc20d2a290daea0d52bdc2ed2ad4be6491010e' into HEAD
        |
        o fe65c1f create test2.txt
        |
        | & (merge) 62fc20d create test1.txt
        |/
        @ 3ebbc8f Merge commit '62fc20d2a290daea0d52bdc2ed2ad4be6491010e' into HEAD
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["show"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit 3ebbc8fdaff7b5d5d0f1101feb3640d06b0297a2
        Merge: fe65c1f 62fc20d
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            Merge commit '62fc20d2a290daea0d52bdc2ed2ad4be6491010e' into HEAD

        diff --cc test1.txt
        index 0000000,7432a8f..2121042
        mode 000000,100644..100644
        --- a/test1.txt
        +++ b/test1.txt
        @@@ -1,0 -1,1 +1,1 @@@
         -test1 contents
        ++new test1 contents
        "###);
    }

    Ok(())
}
