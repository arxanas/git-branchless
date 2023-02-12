pub mod util;

use std::str::FromStr;

use lib::git::NonZeroOid;
use lib::testing::{make_git, trim_lines, GitInitOptions, GitRunOptions};

#[test]
fn test_restore_snapshot_basic() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.write_file_txt("test1", "test1 new contents\n")?;
    git.write_file_txt("test2", "staged contents\n")?;
    git.run(&["add", "test2.txt"])?;

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        On branch master
        Changes to be committed:
          (use "git restore --staged <file>..." to unstage)
        	modified:   test2.txt

        Changes not staged for commit:
          (use "git add <file>..." to update what will be committed)
          (use "git restore <file>..." to discard changes in working directory)
        	modified:   test1.txt

        Changes to be committed:
        diff --git c/test2.txt i/test2.txt
        index 4e512d2..4480ae4 100644
        --- c/test2.txt
        +++ i/test2.txt
        @@ -1 +1 @@
        -test2 contents
        +staged contents
        --------------------------------------------------
        Changes not staged for commit:
        diff --git i/test1.txt w/test1.txt
        index 7432a8f..6cbc96e 100644
        --- i/test1.txt
        +++ w/test1.txt
        @@ -1 +1 @@
        -test1 contents
        +test1 new contents
        "###);
    }

    let original_status = {
        let (stdout, _stderr) = git.run(&["status", "--porcelain=2"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        1 .M N... 100644 100644 100644 7432a8fff25da8f35a9960893ad6155d1d150d39 7432a8fff25da8f35a9960893ad6155d1d150d39 test1.txt
        1 M. N... 100644 100644 100644 4e512d2fd80b9630225ca53f211aeff0544f8b36 4480ae41d60ff497031ec9d48870ed9604477173 test2.txt
        "###);
        stdout
    };

    let snapshot_oid = {
        let (snapshot_oid, stderr) = git.branchless("snapshot", &["create"])?;
        insta::assert_snapshot!(stderr, @"branchless: creating working copy snapshot
");
        NonZeroOid::from_str(snapshot_oid.trim())?
    };

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        On branch master
        nothing to commit, working tree clean
        "###);
    }

    {
        let (stdout, stderr) =
            git.branchless("snapshot", &["restore", &snapshot_oid.to_string()])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stderr, @r###"
        branchless: restoring from snapshot
        branchless: processing 2 updates: branch master, ref HEAD
        branchless: processing 1 update: ref HEAD
        HEAD is now at f7ec40d branchless: working copy snapshot data: 2 unstaged changes
        branchless: processing checkout
        branchless: processing 1 update: ref HEAD
        branchless: processing 1 update: branch master
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset --hard HEAD
        HEAD is now at 96d1c37 create test2.txt
        branchless: running command: <git-executable> checkout f7ec40d081d7fa358f5e283ebf42f06a1508084c
        branchless: running command: <git-executable> reset 96d1c37a3d4363611c49f7e52186e189a04c531f
        Unstaged changes after reset:
        M	test1.txt
        M	test2.txt
        branchless: running command: <git-executable> update-ref refs/heads/master 96d1c37a3d4363611c49f7e52186e189a04c531f
        branchless: running command: <git-executable> symbolic-ref HEAD refs/heads/master
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        On branch master
        Changes to be committed:
          (use "git restore --staged <file>..." to unstage)
        	modified:   test2.txt

        Changes not staged for commit:
          (use "git add <file>..." to update what will be committed)
          (use "git restore <file>..." to discard changes in working directory)
        	modified:   test1.txt

        Changes to be committed:
        diff --git c/test2.txt i/test2.txt
        index 4e512d2..4480ae4 100644
        --- c/test2.txt
        +++ i/test2.txt
        @@ -1 +1 @@
        -test2 contents
        +staged contents
        --------------------------------------------------
        Changes not staged for commit:
        diff --git i/test1.txt w/test1.txt
        index 7432a8f..6cbc96e 100644
        --- i/test1.txt
        +++ w/test1.txt
        @@ -1 +1 @@
        -test1 contents
        +test1 new contents
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status", "--porcelain=2"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        1 .M N... 100644 100644 100644 7432a8fff25da8f35a9960893ad6155d1d150d39 7432a8fff25da8f35a9960893ad6155d1d150d39 test1.txt
        1 M. N... 100644 100644 100644 4e512d2fd80b9630225ca53f211aeff0544f8b36 4480ae41d60ff497031ec9d48870ed9604477173 test2.txt
        "###);
        assert_eq!(original_status, stdout);
    }

    Ok(())
}

#[test]
fn test_restore_snapshot_deleted_files() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.delete_file("test1")?;
    git.run(&["rm", "test2.txt"])?;

    let original_status = {
        let (stdout, _stderr) = git.run(&["status", "--porcelain=2"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        1 .D N... 100644 100644 000000 7432a8fff25da8f35a9960893ad6155d1d150d39 7432a8fff25da8f35a9960893ad6155d1d150d39 test1.txt
        1 D. N... 100644 000000 000000 4e512d2fd80b9630225ca53f211aeff0544f8b36 0000000000000000000000000000000000000000 test2.txt
        "###);
        stdout
    };

    let snapshot_oid = {
        let (snapshot_oid, stderr) = git.branchless("snapshot", &["create"])?;
        insta::assert_snapshot!(stderr, @"branchless: creating working copy snapshot
");
        NonZeroOid::from_str(snapshot_oid.trim())?
    };

    {
        let (stdout, _stderr) = git.run(&["status", "--porcelain=2"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @"");
    }

    {
        let (stdout, stderr) =
            git.branchless("snapshot", &["restore", &snapshot_oid.to_string()])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stderr, @r###"
        branchless: restoring from snapshot
        branchless: processing 2 updates: branch master, ref HEAD
        branchless: processing 1 update: ref HEAD
        HEAD is now at 1935fed branchless: working copy snapshot data: 2 unstaged changes
        branchless: processing checkout
        branchless: processing 1 update: ref HEAD
        branchless: processing 1 update: branch master
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset --hard HEAD
        HEAD is now at 96d1c37 create test2.txt
        branchless: running command: <git-executable> checkout 1935fedb3b0232849e52e44225b2e5bbe9de0ff7
        branchless: running command: <git-executable> reset 96d1c37a3d4363611c49f7e52186e189a04c531f
        Unstaged changes after reset:
        D	test1.txt
        D	test2.txt
        branchless: running command: <git-executable> update-ref refs/heads/master 96d1c37a3d4363611c49f7e52186e189a04c531f
        branchless: running command: <git-executable> symbolic-ref HEAD refs/heads/master
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status", "--porcelain=2"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        1 .D N... 100644 100644 000000 7432a8fff25da8f35a9960893ad6155d1d150d39 7432a8fff25da8f35a9960893ad6155d1d150d39 test1.txt
        1 D. N... 100644 000000 000000 4e512d2fd80b9630225ca53f211aeff0544f8b36 0000000000000000000000000000000000000000 test2.txt
        "###);
        assert_eq!(original_status, stdout);
    }

    Ok(())
}

#[test]
fn test_restore_snapshot_delete_file_only_in_index() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.run(&["rm", "--cached", "test1.txt"])?;

    {
        let (stdout, _stderr) = git.run(&["status", "--porcelain=2"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        1 D. N... 100644 000000 000000 7432a8fff25da8f35a9960893ad6155d1d150d39 0000000000000000000000000000000000000000 test1.txt
        ? test1.txt
        "###);
    }

    let snapshot_oid = {
        let (snapshot_oid, stderr) = git.branchless("snapshot", &["create"])?;
        insta::assert_snapshot!(stderr, @"branchless: creating working copy snapshot
");
        NonZeroOid::from_str(snapshot_oid.trim())?
    };

    {
        let (stdout, _stderr) = git.run(&["status", "--porcelain=2"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @"");
    }

    {
        let (stdout, stderr) =
            git.branchless("snapshot", &["restore", &snapshot_oid.to_string()])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stderr, @r###"
        branchless: restoring from snapshot
        branchless: processing 2 updates: branch master, ref HEAD
        branchless: processing 1 update: ref HEAD
        HEAD is now at eb8b9ee branchless: working copy snapshot data: 1 unstaged change
        branchless: processing checkout
        branchless: processing 1 update: ref HEAD
        branchless: processing 1 update: branch master
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset --hard HEAD
        HEAD is now at 62fc20d create test1.txt
        branchless: running command: <git-executable> checkout eb8b9eecf747c1aa08bb9dd0cbda15b9a90082af
        branchless: running command: <git-executable> reset 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
        Unstaged changes after reset:
        D	test1.txt
        branchless: running command: <git-executable> update-ref refs/heads/master 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
        branchless: running command: <git-executable> symbolic-ref HEAD refs/heads/master
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status", "--porcelain=2"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @"1 D. N... 100644 000000 000000 7432a8fff25da8f35a9960893ad6155d1d150d39 0000000000000000000000000000000000000000 test1.txt
");
    }

    Ok(())
}

#[test]
fn test_restore_snapshot_respect_untracked_changes() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;

    let snapshot_oid = {
        let (snapshot_oid, stderr) = git.branchless("snapshot", &["create"])?;
        insta::assert_snapshot!(stderr, @"branchless: creating working copy snapshot
");
        NonZeroOid::from_str(snapshot_oid.trim())?
    };

    {
        let (stdout, _stderr) = git.run(&["status", "--porcelain=2"])?;
        insta::assert_snapshot!(stdout, @"");
    }

    git.run(&["checkout", "HEAD^"])?;
    git.write_file_txt("test1", "untracked contents")?;

    {
        let (stdout, stderr) = git.run_with_options(
            &[
                "branchless",
                "snapshot",
                "restore",
                &snapshot_oid.to_string(),
            ],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @r###"
        branchless: restoring from snapshot
        branchless: processing 1 update: ref HEAD
        error: The following untracked working tree files would be overwritten by checkout:
        	test1.txt
        Please move or remove them before you switch branches.
        Aborting
        "###);
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset --hard HEAD
        HEAD is now at f777ecc create initial.txt
        branchless: running command: <git-executable> checkout cd8605eef8b78e22427fa3846f1a23f95e88aa7e
        "###);
    }

    Ok(())
}

#[test]
fn test_snapshot_merge_conflict() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file_with_contents("test2", 3, "new test2 contents\n")?;
    git.run(&["checkout", "-b", "change", "HEAD^"])?;
    git.run(&["rm", "test2.txt"])?;
    git.run(&["commit", "-m", "delete test2.txt"])?;

    {
        let (stdout, stderr) = git.run_with_options(
            &["merge", "master"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        assert_ne!(stdout, "");
    }

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        On branch change
        You have unmerged paths.
          (fix conflicts and run "git commit")
          (use "git merge --abort" to abort the merge)

        Unmerged paths:
          (use "git add/rm <file>..." as appropriate to mark resolution)
        	deleted by us:   test2.txt

        * Unmerged path test2.txt
        no changes added to commit (use "git add" and/or "git commit -a")
        "###);
    }

    let snapshot_oid = {
        let (stdout, stderr) = git.branchless("snapshot", &["create"])?;
        insta::assert_snapshot!(stderr, @"branchless: creating working copy snapshot
");
        NonZeroOid::from_str(stdout.trim())?
    };

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        On branch change
        nothing to commit, working tree clean
        "###);
    }

    {
        let (stdout, _stderr) =
            git.branchless("snapshot", &["restore", &snapshot_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset --hard HEAD
        HEAD is now at 588fac3 delete test2.txt
        branchless: running command: <git-executable> checkout 1ea5d7118ef363f3e4ed0ed6c250dd01b91bff2a
        branchless: running command: <git-executable> reset 588fac31cba846f7278a95e1361c45118be90c6c
        branchless: running command: <git-executable> update-ref refs/heads/change 588fac31cba846f7278a95e1361c45118be90c6c
        branchless: running command: <git-executable> symbolic-ref HEAD refs/heads/change
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        On branch change
        Unmerged paths:
          (use "git restore --staged <file>..." to unstage)
          (use "git add/rm <file>..." as appropriate to mark resolution)
        	deleted by us:   test2.txt

        * Unmerged path test2.txt
        no changes added to commit (use "git add" and/or "git commit -a")
        "###);
    }

    Ok(())
}

#[test]
fn test_snapshot_restore_unborn_head() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo_with_options(&GitInitOptions {
        make_initial_commit: false,
        run_branchless_init: false,
    })?;

    git.branchless("init", &["--main-branch", "master"])?;

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        On branch master

        No commits yet

        nothing to commit (create/copy files and use "git add" to track)
        "###);
    }

    let snapshot_oid = {
        let (snapshot_oid, stderr) = git.branchless("snapshot", &["create"])?;
        insta::assert_snapshot!(stderr, @"branchless: creating working copy snapshot
");
        NonZeroOid::from_str(snapshot_oid.trim())?
    };

    git.commit_file("test1", 1)?;
    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @"@ 6118a39 (> master) create test1.txt
");
    }

    {
        let (stdout, _stderr) =
            git.branchless("snapshot", &["restore", &snapshot_oid.to_string()])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> reset --hard HEAD
        HEAD is now at 6118a39 create test1.txt
        branchless: running command: <git-executable> checkout 20939f1f30f51ffaa6569d218cd7a50f24c956cf
        branchless: running command: <git-executable> update-ref refs/heads/master 0000000000000000000000000000000000000000
        branchless: running command: <git-executable> symbolic-ref HEAD refs/heads/master
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["status", "-vv"])?;
        insta::assert_snapshot!(stdout, @r###"
        On branch master

        No commits yet

        nothing to commit (create/copy files and use "git add" to track)
        "###);
    }

    Ok(())
}
