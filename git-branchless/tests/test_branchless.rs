use std::collections::HashMap;

use itertools::Itertools;
use lib::testing::{make_git, GitRunOptions};

#[test]
fn test_commands() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test", 1)?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 3df4b93 (> master) create test.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["hide", "3df4b935"])?;
        insta::assert_snapshot!(stdout, @r###"
        Hid commit: 3df4b93 create test.txt
        Abandoned 1 branch: master
        To unhide this 1 commit, run: git undo
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["unhide", "3df4b935"])?;
        insta::assert_snapshot!(stdout, @r###"
        Unhid commit: 3df4b93 create test.txt
        To hide this 1 commit, run: git undo
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["prev"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout f777ecc9b0db5ed372b2615695191a8a17f79f24
        @ f777ecc create initial.txt
        |
        O 3df4b93 (master) create test.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["next"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 3df4b9355b3b072aa6c50c6249bf32e289b3a661
        :
        @ 3df4b93 (master) create test.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_profiling() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.run_with_options(
        &["smartlog"],
        &GitRunOptions {
            env: {
                let mut env: HashMap<String, String> = HashMap::new();
                env.insert("RUST_PROFILE".to_string(), "1".to_string());
                env
            },
            ..Default::default()
        },
    )?;

    let entries: Vec<_> = std::fs::read_dir(&git.repo_path)?
        .into_iter()
        .try_collect()?;
    assert!(entries
        .iter()
        .any(|entry| entry.file_name().to_str().unwrap().contains("trace-")));

    Ok(())
}

#[test]
fn test_sparse_checkout() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    if git.run(&["sparse-checkout", "set"]).is_err() {
        return Ok(());
    }

    {
        let (stdout, _stderr) = git.run(&["config", "extensions.worktreeConfig"])?;
        insta::assert_snapshot!(stdout, @r###"
        true
        "###);
    }

    {
        let (stdout, _stderr) = git.run_with_options(
            &["smartlog"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        Error: the Git configuration setting `extensions.worktreeConfig` is enabled in
        this repository. Due to upstream libgit2 limitations, git-branchless does not
        support repositories with this configuration option enabled.

        Usually, this configuration setting is enabled when initializing a sparse
        checkout. See https://github.com/arxanas/git-branchless/issues/278 for more
        information.

        Here are some options:

        - To unset the configuration option, run: git config --unset extensions.worktreeConfig
          - This is safe unless you created another worktree also using a sparse checkout.
        - Try upgrading to Git v2.36+ and reinitializing your sparse checkout.
        "###);
    }

    Ok(())
}

#[test]
fn test_core_split_index() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.run(&["config", "core.splitIndex", "true"])?;

    {
        let (stdout, stderr) = git.run(&["update-index", "--split-index"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @"");
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc (> master) create initial.txt
        "###);
    }

    Ok(())
}
