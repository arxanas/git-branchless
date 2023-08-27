use branchless::git::{GitRunInfo, GitRunOpts};
use branchless::testing::make_git;

#[test]
fn test_hook_working_dir() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.commit_file("test1", 1)?;

    std::fs::write(
        git.repo_path
            .join(".git")
            .join("hooks")
            .join("post-rewrite"),
        r#"#!/bin/sh
                   # This won't work unless we're running the hook in the Git working copy.
                   echo "Check if test1.txt exists"
                   [ -f test1.txt ] && echo "test1.txt exists"
                   "#,
    )?;

    {
        // Trigger the `post-rewrite` hook that we wrote above.
        let (stdout, stderr) = git.run(&["commit", "--amend", "-m", "foo"])?;
        insta::assert_snapshot!(stderr, @r###"
            branchless: processing 2 updates: branch master, ref HEAD
            branchless: processed commit: f23bf8f foo
            Check if test1.txt exists
            test1.txt exists
            "###);
        insta::assert_snapshot!(stdout, @r###"
                [master f23bf8f] foo
                 Date: Thu Oct 29 12:34:56 2020 -0100
                 1 file changed, 1 insertion(+)
                 create mode 100644 test1.txt
                "###);
    }

    Ok(())
}

#[test]
fn test_run_silent_failures() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    let git_run_info = GitRunInfo {
        path_to_git: git.path_to_git.clone(),
        working_directory: git.repo_path.clone(),
        env: Default::default(),
    };

    let result = git_run_info.run_silent(
        &git.get_repo()?,
        None,
        &["some-nonexistent-command"],
        GitRunOpts {
            treat_git_failure_as_error: true,
            stdin: None,
        },
    );
    assert!(result.is_err());

    let result = git_run_info.run_silent(
        &git.get_repo()?,
        None,
        &["some-nonexistent-command"],
        GitRunOpts {
            treat_git_failure_as_error: false,
            stdin: None,
        },
    );
    assert!(result.is_ok());

    Ok(())
}

// Creating symlinks on Windows may fail without administrator or developer
// privileges, so this test is Unix only. See
// https://doc.rust-lang.org/std/os/windows/fs/fn.symlink_dir.html#limitations
// for more details.
#[cfg(unix)]
#[test]
fn test_run_in_repo_tool_project() -> eyre::Result<()> {
    use std::{fs, os::unix};

    let git = make_git()?;
    git.init_repo()?;

    let git_dir = git.repo_path.join(".git");
    let repo_managed_dir = tempfile::tempdir()?;
    let repo_managed_git_dir = repo_managed_dir.path().join(".repo/test_repo");
    fs::create_dir_all(&repo_managed_git_dir)?;
    fs::rename(&git_dir, &repo_managed_git_dir)?;

    unix::fs::symlink(repo_managed_git_dir, git_dir)?;

    let git_run_info = GitRunInfo {
        path_to_git: git.path_to_git.clone(),
        working_directory: git.repo_path.clone(),
        env: Default::default(),
    };

    let result = git_run_info.run_silent(
        &git.get_repo()?,
        None,
        &["status"],
        GitRunOpts {
            treat_git_failure_as_error: true,
            stdin: None,
        },
    );
    assert!(result.is_ok());

    Ok(())
}
