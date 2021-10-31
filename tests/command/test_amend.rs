use branchless::testing::{make_git, GitRunOptions};

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
        [1/1] Committed as: b51f01b6 create test3.txt
        branchless: processing 1 rewritten commit
        In-memory rebase succeeded.
        Finished restacking commits.
        No abandoned branches to restack.
        branchless: running command: <git-executable> checkout 7ac317b9d1dd1bbdf46e8ee692b9b9e280f28a50
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        @ 7ac317b9 create test2.txt
        |
        o b51f01b6 create test3.txt
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
        branchless: running command: <git-executable> diff --quiet
        Attempting rebase in-memory...
        This operation would cause a merge conflict:
        - (1 conflicting file) b51f01b6 create test3.txt
        To resolve merge conflicts, retry this operation with the --merge option.
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
        branchless: running command: <git-executable> diff --quiet
        No abandoned commits to restack.
        No abandoned branches to restack.
        branchless: running command: <git-executable> checkout f6b255388219264f4bcd258a3020d262c2d7b03e
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        @ f6b25538 create test2.txt
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
        branchless: running command: <git-executable> checkout f0f07277a6448cac370e6023ab379ec0c601ccfe
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        @ f0f07277 create test2.txt
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
        branchless: running command: <git-executable> diff --quiet
        No abandoned commits to restack.
        No abandoned branches to restack.
        branchless: running command: <git-executable> checkout f8e4ba1be5cefcf22e831f51b1525b0be8215a31
        M	test2.txt
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        @ f8e4ba1b create test2.txt
        Amended with 1 staged change. (Some uncommitted changes were not amended.)
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["branchless", "amend"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset
        No abandoned commits to restack.
        No abandoned branches to restack.
        branchless: running command: <git-executable> checkout 2e69581cb466962fa85e5918f29af6d2925fdd6f
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        @ 2e69581c create test2.txt
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
        branchless: running command: <git-executable> checkout 3b98a960e6ebde39a933c25413b43bce8c0fd128
        O f777ecc9 (master) create initial.txt
        |
        @ 3b98a960 create test1.txt
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
        branchless: running command: <git-executable> diff --quiet
        No abandoned commits to restack.
        No abandoned branches to restack.
        branchless: running command: <git-executable> checkout 685ef311b070a460b7c86a9aed068be563978021
        O f777ecc9 (master) create initial.txt
        |
        @ 685ef311 create test1.txt
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
        branchless: running command: <git-executable> diff --quiet
        No abandoned commits to restack.
        No abandoned branches to restack.
        branchless: running command: <git-executable> checkout f00ec4b5a81438f4e792ca5576a290b16fed8fdb
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        @ f00ec4b5 create test2.txt
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
