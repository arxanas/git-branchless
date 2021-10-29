use regex::Regex;

use crate::util::trim_lines;

use branchless::git::GitVersion;
use branchless::testing::{make_git, GitInitOptions, GitRunOptions};
use eyre::Context;

#[test]
fn test_hook_installed() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    let hook_path = git.repo_path.join(".git").join("hooks").join("post-commit");
    assert!(hook_path.exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(&hook_path)
            .wrap_err_with(|| format!("Reading hook permissions for {:?}", &hook_path))?;
        let mode = metadata.permissions().mode();
        assert!(mode & 0o111 == 0o111);
    }

    Ok(())
}

#[test]
fn test_hook_appended_to_existing_contents() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    let hook_path = git.repo_path.join(".git").join("hooks").join("post-commit");
    std::fs::write(
        &hook_path,
        "#!/bin/sh
echo Hello, world
",
    )?;

    git.run(&["branchless", "init"])?;

    {
        let (stdout, stderr) = git.run(&["commit", "--allow-empty", "-m", "test"])?;
        insta::assert_snapshot!(stdout, @"[master 4cd1a9b] test
");
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 2 updates: branch master, ref HEAD
        Hello, world
        branchless: processed commit: 4cd1a9ba test
        "###);
    }

    Ok(())
}

#[test]
fn test_alias_installed() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
@ f777ecc9 (master) create initial.txt
"###);
    }

    {
        let (stdout, _stderr) = git.run(&["sl"])?;
        insta::assert_snapshot!(stdout, @r###"
@ f777ecc9 (master) create initial.txt
"###);
    }

    Ok(())
}

#[test]
fn test_old_git_version_warning() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    let version = git.get_version()?;
    if version < GitVersion(2, 29, 0) {
        let (stdout, _stderr) = git.run(&["branchless", "init"])?;
        let (version_str, _stderr) = git.run(&["version"])?;
        let stdout = stdout.replace(version_str.trim(), "<git version output>");
        insta::assert_snapshot!(stdout, @r###"
        Created config file at <repo-path>/.git/branchless/config
        Auto-detected your main branch as: master
        If this is incorrect, run: git config branchless.core.mainBranch <branch>
        Installing hook: post-commit
        Installing hook: post-merge
        Installing hook: post-rewrite
        Installing hook: post-checkout
        Installing hook: pre-auto-gc
        Installing hook: reference-transaction
        Warning: the branchless workflow's `git undo` command requires Git
        v2.29 or later, but your Git version is: <git version output>

        Some operations, such as branch updates, won't be correctly undone. Other
        operations may be undoable. Attempt at your own risk.

        Once you upgrade to Git v2.29, run `git branchless init` again. Any work you
        do from then on will be correctly undoable.

        This only applies to the `git undo` command. Other commands which are part of
        the branchless workflow will work properly.
        Successfully installed git-branchless.
        To uninstall, run: git branchless init --uninstall
        "###);
    }

    Ok(())
}

#[cfg(unix)]
#[test]
fn test_init_basic() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo_with_options(&GitInitOptions {
        run_branchless_init: false,
        ..Default::default()
    })?;

    {
        let (stdout, _stderr) = git.run(&["branchless", "init"])?;
        insta::assert_snapshot!(stdout, @r###"
        Created config file at <repo-path>/.git/branchless/config
        Auto-detected your main branch as: master
        If this is incorrect, run: git config branchless.core.mainBranch <branch>
        Installing hook: post-commit
        Installing hook: post-merge
        Installing hook: post-rewrite
        Installing hook: post-checkout
        Installing hook: pre-auto-gc
        Installing hook: reference-transaction
        Successfully installed git-branchless.
        To uninstall, run: git branchless init --uninstall
        "###);
    }

    Ok(())
}

#[cfg(unix)]
#[test]
fn test_init_prompt_for_main_branch() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo_with_options(&GitInitOptions {
        run_branchless_init: false,
        ..Default::default()
    })?;

    git.run(&["branch", "-m", "master", "bespoke"])?;

    {
        let (stdout, stderr) = git.run_with_options(
            &["branchless", "init"],
            &GitRunOptions {
                input: Some("bespoke\n".to_string()),
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Created config file at <repo-path>/.git/branchless/config
        Your main branch name could not be auto-detected!
        Examples of a main branch: master, main, trunk, etc.
        See https://github.com/arxanas/git-branchless/wiki/Concepts#main-branch
        Enter the name of your main branch: Installing hook: post-commit
        Installing hook: post-merge
        Installing hook: post-rewrite
        Installing hook: post-checkout
        Installing hook: pre-auto-gc
        Installing hook: reference-transaction
        Successfully installed git-branchless.
        To uninstall, run: git branchless init --uninstall
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @"@ f777ecc9 (bespoke) create initial.txt
");
    }

    Ok(())
}

#[cfg(unix)]
#[test]
fn test_main_branch_not_found_error_message() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.run(&["branch", "-d", "master"])?;

    let (stdout, stderr) = git.run_with_options(
        &["smartlog"],
        &GitRunOptions {
            // Exit code 101 indicates a panic.
            expected_exit_code: 101,

            ..Default::default()
        },
    )?;

    let location_trace_re = Regex::new(r"[^ .]+\.rs:[0-9]+")?;
    let stderr = trim_lines(stderr);
    let stderr = console::strip_ansi_codes(&stderr);
    let stderr = location_trace_re.replace_all(&stderr, "some/file/path.rs:123");
    insta::assert_snapshot!(stderr, @r###"
    The application panicked (crashed).
    Message:  A fatal error occurred:
       0: Could not find repository main branch

    Location:
       some/file/path.rs:123

      ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ SPANTRACE ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

       0: branchless::git::repo::get_main_branch_oid with self=<Git repository at: "<repo-path>/.git/">
          at some/file/path.rs:123
       1: branchless::commands::smartlog::smartlog with effects=<Output fancy=false> options=SmartlogOptions { show_hidden_commits: false }
          at some/file/path.rs:123

    Suggestion:
    The main branch "master" could not be found in your repository
    at path: "<repo-path>/.git/".
    These branches exist: []
    Either create it, or update the main branch setting by running:

        git config branchless.core.mainBranch <branch>


    Backtrace omitted.
    Run with RUST_BACKTRACE=1 environment variable to display it.
    Run with RUST_BACKTRACE=full to include source snippets.
    Location: some/file/path.rs:123

    Backtrace omitted.
    Run with RUST_BACKTRACE=1 environment variable to display it.
    Run with RUST_BACKTRACE=full to include source snippets.
    "###);
    insta::assert_snapshot!(stdout, @"");

    Ok(())
}

#[cfg(unix)]
#[test]
fn test_init_uninstall() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    {
        let (stdout, stderr) = git.run(&["branchless", "init", "--uninstall"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Removing config file: <repo-path>/.git/branchless/config
        Uninstalling hook: post-commit
        Uninstalling hook: post-merge
        Uninstalling hook: post-rewrite
        Uninstalling hook: post-checkout
        Uninstalling hook: pre-auto-gc
        Uninstalling hook: reference-transaction
        "###);
    }

    Ok(())
}

#[cfg(feature = "man-pages")]
#[test]
fn test_man_viewer_installed() -> eyre::Result<()> {
    use std::collections::HashMap;

    use itertools::Itertools;

    // The `man` executable isn't installed for most Windows Git installations.
    // In particular, it's not installed on Github Actions.  It might be
    // possible to install it manually, but I didn't bother.
    //
    // See https://stackoverflow.com/q/5517564,
    // https://github.com/swcarpentry/shell-novice/issues/249
    let should_skip = cfg!(windows);
    if should_skip {
        return Ok(());
    }

    let git = make_git()?;
    git.init_repo()?;

    // `env` and `man` are not on the sanitized testing `PATH`, so use the
    // caller's `PATH` instead.
    let testing_path = git.get_path_for_env();
    let testing_path = std::env::split_paths(&testing_path).collect_vec();
    let inherited_path = std::env::var_os("PATH").unwrap();
    let inherited_path = std::env::split_paths(&inherited_path).collect_vec();
    let env = {
        let mut env = HashMap::new();
        let full_path = std::env::join_paths(testing_path.iter().chain(inherited_path.iter()))?;
        let full_path = full_path.to_str().unwrap().to_owned();
        env.insert("PATH".to_string(), full_path);
        env
    };

    {
        let (stdout, _stderr) = git.run_with_options(
            &["smartlog", "--help"],
            &GitRunOptions {
                env: env.clone(),
                ..Default::default()
            },
        )?;
        let first_word = stdout.split_whitespace().next();
        insta::assert_debug_snapshot!(first_word, @r###"
        Some(
            "GIT-BRANCHLESS-SMARTLOG(1)",
        )
        "###);
    }

    {
        let (stdout, _stderr) = git.run_with_options(
            &["init", "--help"],
            &GitRunOptions {
                env,
                ..Default::default()
            },
        )?;
        let first_word = stdout.split_whitespace().next();
        insta::assert_debug_snapshot!(first_word, @r###"
        Some(
            "GIT-INIT(1)",
        )
        "###);
    }

    Ok(())
}

#[test]
fn test_init_explicit_main_branch_name() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo_with_options(&GitInitOptions {
        run_branchless_init: false,
        ..Default::default()
    })?;

    {
        // Set the default branch to ensure `--main-branch` takes precedence
        // over the repo default.
        git.run(&["config", "init.defaultBranch", "repo-default-branch"])?;

        git.run(&["branchless", "init", "--main-branch", "foo"])?;
        git.run(&["checkout", "-b", "foo"])?;
        git.run(&["branch", "-d", "master"])?;
        git.commit_file("test1", 1)?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d2 (foo) create test1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_init_repo_default_branch() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo_with_options(&GitInitOptions {
        run_branchless_init: false,
        ..Default::default()
    })?;

    {
        git.run(&["checkout", "-b", "repo-default-branch"])?;
        git.run(&["config", "init.defaultBranch", "repo-default-branch"])?;
        git.run(&["branch", "-d", "master"])?;

        git.run(&["branchless", "init"])?;
        git.commit_file("test1", 1)?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d2 (repo-default-branch) create test1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_hide_branchless_refs_from_git_log() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_log_exclude_decoration()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.run(&["update-ref", "refs/foo/bar", "HEAD"])?;

    {
        let (stdout, _stderr) = git.run(&["log", "--decorate"])?;
        insta::assert_snapshot!(stdout, @r###"
        commit 62fc20d2a290daea0d52bdc2ed2ad4be6491010e (HEAD -> master, refs/foo/bar)
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 -0100

            create test1.txt

        commit f777ecc9b0db5ed372b2615695191a8a17f79f24
        Author: Testy McTestface <test@example.com>
        Date:   Thu Oct 29 12:34:56 2020 +0000

            create initial.txt
        "###);
    }

    Ok(())
}
