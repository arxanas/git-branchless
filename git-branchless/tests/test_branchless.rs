use std::collections::HashMap;

use git_branchless_testing::{make_git, GitRunOptions};
use itertools::Itertools;

#[test]
fn test_commands() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test", 1)?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 3df4b93 (> master) create test.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("hide", &["3df4b935"])?;
        insta::assert_snapshot!(stdout, @r###"
        Hid commit: 3df4b93 create test.txt
        Abandoned 1 branch: master
        To unhide this 1 commit, run: git undo
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("unhide", &["3df4b935"])?;
        insta::assert_snapshot!(stdout, @r###"
        Unhid commit: 3df4b93 create test.txt
        To hide this 1 commit, run: git undo
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("prev", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout f777ecc9b0db5ed372b2615695191a8a17f79f24
        @ f777ecc create initial.txt
        |
        O 3df4b93 (master) create test.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless("next", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout master
        :
        @ 3df4b93 (> master) create test.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_profiling() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.branchless_with_options(
        "smartlog",
        &[],
        &GitRunOptions {
            env: {
                let mut env: HashMap<String, String> = HashMap::new();
                env.insert("RUST_PROFILE".to_string(), "1".to_string());
                env
            },
            ..Default::default()
        },
    )?;

    let entries: Vec<_> = std::fs::read_dir(&git.repo_path)?.try_collect()?;
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

    if let Ok(stdout) = git.smartlog() {
        insta::assert_snapshot!(stdout, @"@ f777ecc (> master) create initial.txt
");
    } else {
        let (stdout, _stderr) = git.branchless_with_options(
            "smartlog",
            &[],
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
