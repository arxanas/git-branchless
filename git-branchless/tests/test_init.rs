use std::collections::HashMap;

use eyre::Context;
use itertools::Itertools;
use lib::git::GitVersion;
use lib::testing::{
    make_git, make_git_worktree, GitInitOptions, GitRunOptions, GitWorktreeWrapper,
};
use regex::Regex;

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
        hook_path,
        "#!/bin/sh
echo Hello, world
",
    )?;

    git.branchless("init", &[])?;

    {
        let (stdout, stderr) = git.run(&["commit", "--allow-empty", "-m", "test"])?;
        insta::assert_snapshot!(stdout, @"[master 4cd1a9b] test
");
        insta::assert_snapshot!(stderr, @r###"
        branchless: processing 2 updates: branch master, ref HEAD
        Hello, world
        branchless: processed commit: 4cd1a9b test
        "###);
    }

    Ok(())
}

#[test]
fn test_alias_installed() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @"@ f777ecc (> master) create initial.txt
");
    }

    {
        let (stdout, _stderr) = git.run(&["sl"])?;
        insta::assert_snapshot!(stdout, @"@ f777ecc (> master) create initial.txt
");
    }

    Ok(())
}

#[test]
fn test_dont_install_existing_aliases() -> eyre::Result<()> {
    let git = make_git()?;

    let git_init_options = GitInitOptions {
        run_branchless_init: false,
        ..Default::default()
    };

    git.init_repo_with_options(&git_init_options)?;

    // Create a fake $HOME directory, to allow us to emulate a user's .gitconfig file
    let fake_home_dir = git.repo_path.join("fake_home");
    let fake_home_git_config = fake_home_dir.join(".gitconfig");
    std::fs::create_dir(&fake_home_dir)?;
    std::fs::write(&fake_home_git_config, "[alias]\n\tsl = status\n")?;

    let env = HashMap::from([(
        "HOME".to_string(),
        fake_home_dir.to_string_lossy().to_string(),
    )]);
    let git_run_options = GitRunOptions {
        env,
        ..GitRunOptions::default()
    };

    // Initialize branchless and make sure it didn't add the "sl" alias
    git.branchless_with_options("init", &[], &git_run_options)?;

    {
        let (expected_stdout, _) = git.run_with_options(&["status"], &git_run_options)?;
        let (actual_stdout, _) = git.run_with_options(&["sl"], &git_run_options)?;

        assert_eq!(expected_stdout, actual_stdout);
    }

    // Update the config to add "smartlog" and make sure neither alias is added
    std::fs::write(
        &fake_home_git_config,
        "[alias]\n\tsl = status\n\tsmartlog = status",
    )?;

    git.branchless_with_options("init", &[], &git_run_options)?;

    {
        let (expected_stdout, _) = git.run_with_options(&["status"], &git_run_options)?;
        let (actual_stdout, _) = git.run_with_options(&["sl"], &git_run_options)?;

        assert_eq!(expected_stdout, actual_stdout);
    }

    {
        let (expected_stdout, _) = git.run_with_options(&["status"], &git_run_options)?;
        let (actual_stdout, _) = git.run_with_options(&["smartlog"], &git_run_options)?;

        assert_eq!(expected_stdout, actual_stdout);
    }

    // Update the config to remove both aliases and make sure both are added
    std::fs::write(&fake_home_git_config, "")?;

    git.branchless_with_options("init", &[], &git_run_options)?;

    {
        let (expected_stdout, _) = git.run_with_options(&["smartlog"], &git_run_options)?;
        let (actual_stdout, _) = git.run_with_options(&["sl"], &git_run_options)?;

        assert_eq!(expected_stdout, actual_stdout);
    }

    {
        let (expected_stdout, _) = git.run_with_options(&["status"], &git_run_options)?;
        let (actual_stdout, _) = git.run_with_options(&["sl"], &git_run_options)?;

        assert_ne!(expected_stdout, actual_stdout);
    }

    {
        let (expected_stdout, _) = git.run_with_options(&["status"], &git_run_options)?;
        let (actual_stdout, _) = git.run_with_options(&["smartlog"], &git_run_options)?;

        assert_ne!(expected_stdout, actual_stdout);
    }

    Ok(())
}

#[test]
fn test_old_git_version_warning() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    let version = git.get_version()?;
    if version < GitVersion(2, 29, 0) {
        let (stdout, _stderr) = git.branchless("init", &[])?;
        let (version_str, _stderr) = git.run(&["version"])?;
        let stdout = stdout.replace(version_str.trim(), "<git version output>");
        insta::assert_snapshot!(stdout, @r###"
        Created config file at <repo-path>/.git/branchless/config
        Auto-detected your main branch as: master
        If this is incorrect, run: git branchless init --main-branch <branch>
        Installing hooks: post-applypatch, post-checkout, post-commit, post-merge, post-rewrite, pre-auto-gc, reference-transaction
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
        let (stdout, stderr) = git.branchless("init", &[])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Created config file at <repo-path>/.git/branchless/config
        Auto-detected your main branch as: master
        If this is incorrect, run: git branchless init --main-branch <branch>
        Installing hooks: post-applypatch, post-checkout, post-commit, post-merge, post-rewrite, pre-auto-gc, reference-transaction
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
        let (stdout, stderr) = git.branchless_with_options(
            "init",
            &[],
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
        Enter the name of your main branch: Installing hooks: post-applypatch, post-checkout, post-commit, post-merge, post-rewrite, pre-auto-gc, reference-transaction
        Successfully installed git-branchless.
        To uninstall, run: git branchless init --uninstall
        "###);
    }

    {
        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @"@ f777ecc (> bespoke) create initial.txt
");
    }

    Ok(())
}

#[cfg(unix)]
#[test]
fn test_main_branch_not_found_error_message() -> eyre::Result<()> {
    use lib::testing::trim_lines;

    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.run(&["branch", "-d", "master"])?;

    let (stdout, stderr) = git.branchless_with_options(
        "smartlog",
        &[],
        &GitRunOptions {
            // Exit code 101 indicates a panic.
            expected_exit_code: 101,

            ..Default::default()
        },
    )?;

    let location_trace_re = Regex::new(r"[^ ]+\.rs:[0-9]+")?;
    let stderr = trim_lines(stderr);
    let stderr = console::strip_ansi_codes(&stderr);
    let stderr = location_trace_re.replace_all(&stderr, "some/file/path.rs:123");
    insta::assert_snapshot!(stderr, @r#"
    The application panicked (crashed).
    Message:  A fatal error occurred:
       0: Could not find repository main branch

    Location:
       some/file/path.rs:123

      ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ SPANTRACE ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

       0: branchless::core::eventlog::from_event_log_db with effects=<Output fancy=false> repo=<Git repository at: "<repo-path>/.git/"> event_log_db=<EventLogDb path=Some("<repo-path>/.git/branchless/db.sqlite3")>
          at some/file/path.rs:123
       1: git_branchless_smartlog::smartlog with effects=<Output fancy=false> git_run_info=<GitRunInfo path_to_git="<git-executable>" working_directory="<repo-path>" env=not shown> options=SmartlogOptions { event_id: None, revset: None, resolve_revset_options: ResolveRevsetOptions { show_hidden_commits: false }, reverse: false, exact: false, show_signature: false }
          at some/file/path.rs:123
       2: git_branchless_smartlog::command_main with ctx=CommandContext { effects: <Output fancy=false>, git_run_info: <GitRunInfo path_to_git="<git-executable>" working_directory="<repo-path>" env=not shown> } args=SmartlogArgs { event_id: None, revset: None, reverse: false, exact: false, resolve_revset_options: ResolveRevsetOptions { show_hidden_commits: false }, show_signature: false }
          at some/file/path.rs:123

    Suggestion:
    The main branch "master" could not be found in your repository
    at path: "<repo-path>/.git/".
    These branches exist: []
    Either create it, or update the main branch setting by running:

        git branchless init --main-branch <branch>

    Note that remote main branches are no longer supported as of v0.6.0. See
    https://github.com/arxanas/git-branchless/discussions/595 for more details.

    Backtrace omitted. Run with RUST_BACKTRACE=1 environment variable to display it.
    Run with RUST_BACKTRACE=full to include source snippets.
    Location: some/file/path.rs:123

    Backtrace omitted. Run with RUST_BACKTRACE=1 environment variable to display it.
    Run with RUST_BACKTRACE=full to include source snippets.
    "#);
    insta::assert_snapshot!(stdout, @"");

    Ok(())
}

#[cfg(unix)]
#[test]
fn test_init_uninstall() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    {
        let (stdout, stderr) = git.branchless("init", &["--uninstall"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
        Removing config file: <repo-path>/.git/branchless/config
        Uninstalling hooks: post-applypatch, post-checkout, post-commit, post-merge, post-rewrite, pre-auto-gc, reference-transaction
        "###);
    }

    Ok(())
}

#[test]
fn test_help_flag() -> eyre::Result<()> {
    // NOTE(arxanas, 2024-09-07): Not sure if this test actually fails on
    // Windows since it's no longer exercising the `man` code path.
    //
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

    // NOTE(arxanas, 2024-09-07): This test no longer exercises the man viewer
    // code path, so the below environment manipulation probably does nothing.
    //
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
        let (stdout, _stderr) = git.branchless_with_options(
            "smartlog",
            &["--help"],
            &GitRunOptions {
                env: env.clone(),
                ..Default::default()
            },
        )?;
        let first_line = stdout.lines().next();
        insta::assert_debug_snapshot!(first_line, @r###"
        Some(
            "`smartlog` command",
        )
        "###);
    }

    {
        let (stdout, _stderr) = git.branchless_with_options(
            "init",
            &["--help"],
            &GitRunOptions {
                env,
                ..Default::default()
            },
        )?;
        let first_line = stdout.lines().next();
        insta::assert_debug_snapshot!(first_line, @r###"
        Some(
            "Initialize the branchless workflow for this repository",
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

        git.branchless("init", &["--main-branch", "foo"])?;
        git.run(&["checkout", "-b", "foo"])?;
        git.run(&["branch", "-d", "master"])?;
        git.commit_file("test1", 1)?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d (> foo) create test1.txt
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

        git.branchless("init", &[])?;
        git.commit_file("test1", 1)?;

        let stdout = git.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d (> repo-default-branch) create test1.txt
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

#[cfg(unix)]
#[test]

fn test_init_core_hooks_path_warning() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    let hooks_path = git.get_repo()?.get_path().join("my-hooks");
    std::fs::create_dir_all(hooks_path)?;
    git.run(&["config", "core.hooksPath", "my-hooks"])?;

    {
        let (stdout, _stderr) = git.branchless("init", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        Created config file at <repo-path>/.git/branchless/config
        Auto-detected your main branch as: master
        If this is incorrect, run: git branchless init --main-branch <branch>
        Installing hooks: post-applypatch, post-checkout, post-commit, post-merge, post-rewrite, pre-auto-gc, reference-transaction
        Warning: the configuration value core.hooksPath was set to: my-hooks,
        which is not the expected default value of: <repo-path>/.git/hooks
        The Git hooks above may have been installed to an unexpected global location.
        Successfully installed git-branchless.
        To uninstall, run: git branchless init --uninstall
        "###);
    }

    Ok(())
}

#[cfg(unix)]
#[test]

fn test_init_dynamic_hooks_path_warning() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    if git.get_version()? < GitVersion(2, 36, 0) {
        // `hasconfig` was introduced in Git v2.36.0.
        return Ok(());
    }
    git.init_repo()?;

    let hooks_path = git.get_repo()?.get_path().join("my-hooks");
    std::fs::create_dir_all(&hooks_path)?;
    git.write_file(
        ".git/config",
        r#"
[includeIf "hasconfig:remote.*.url:*"]
path = my-hooks/config

[remote.origin]
url = "hello"
"#,
    )?;
    git.write_file(
        hooks_path.join("config").to_str().unwrap(),
        "
[core]
hooksPath = my-hooks
",
    )?;

    {
        let (stdout, _stderr) = git.branchless("init", &[])?;
        insta::assert_snapshot!(stdout, @r###"
        Created config file at <repo-path>/.git/branchless/config
        Auto-detected your main branch as: master
        If this is incorrect, run: git branchless init --main-branch <branch>
        Installing hooks: post-applypatch, post-checkout, post-commit, post-merge, post-rewrite, pre-auto-gc, reference-transaction
        Warning: the configuration value core.hooksPath was set to: my-hooks,
        which is not the expected default value of: <repo-path>/.git/hooks
        The Git hooks above may have been installed to an unexpected global location.
        Successfully installed git-branchless.
        To uninstall, run: git branchless init --uninstall
        "###);
    }

    Ok(())
}

#[test]
fn test_init_worktree() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo_with_options(&GitInitOptions {
        run_branchless_init: false,
        make_initial_commit: true,
    })?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    let GitWorktreeWrapper {
        temp_dir: _temp_dir,
        worktree,
    } = make_git_worktree(&git, "new-worktree")?;
    worktree.branchless("init", &[])?;
    {
        let stdout = worktree.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 96d1c37 (master) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_install_man_pages() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    let dir = "foo";
    git.branchless("install-man-pages", &[dir])?;
    let man_page_contents = std::fs::read(
        git.repo_path
            .join(dir)
            .join("man1")
            .join("git-branchless.1"),
    )?;
    let man_page_contents = String::from_utf8_lossy(&man_page_contents);
    insta::assert_snapshot!(man_page_contents, @r###"
    .ie \n(.g .ds Aq \(aq
    .el .ds Aq '
    .TH git-branchless 1  "git-branchless 0.10.0" 
    .SH NAME
    git\-branchless \- Branchless workflow for Git
    .SH SYNOPSIS
    \fBgit\-branchless\fR [\fB\-C \fR] [\fB\-\-color\fR] [\fB\-h\fR|\fB\-\-help\fR] [\fB\-V\fR|\fB\-\-version\fR] <\fIsubcommands\fR>
    .SH DESCRIPTION
    Branchless workflow for Git.
    .PP
    See the documentation at https://github.com/arxanas/git\-branchless/wiki.
    .SH OPTIONS
    .TP
    \fB\-C\fR=\fIWORKING_DIRECTORY\fR
    Change to the given directory before executing the rest of the program. (The option is called `\-C` for symmetry with Git.)
    .TP
    \fB\-\-color\fR=\fICOLOR\fR
    Flag to force enable or disable terminal colors
    .br

    .br
    \fIPossible values:\fR
    .RS 14
    .IP \(bu 2
    auto: Automatically determine whether to display colors from the terminal and environment variables. This is the default behavior
    .IP \(bu 2
    always: Always display terminal colors
    .IP \(bu 2
    never: Never display terminal colors
    .RE
    .TP
    \fB\-h\fR, \fB\-\-help\fR
    Print help (see a summary with \*(Aq\-h\*(Aq)
    .TP
    \fB\-V\fR, \fB\-\-version\fR
    Print version
    .SH SUBCOMMANDS
    .TP
    git\-branchless\-amend(1)
    Amend the current HEAD commit
    .TP
    git\-branchless\-bug\-report(1)
    Gather information about recent operations to upload as part of a bug report
    .TP
    git\-branchless\-difftool(1)
    Use the partial commit selector UI as a Git\-compatible difftool; see git\-difftool(1) for more information on Git difftools
    .TP
    git\-branchless\-gc(1)
    Run internal garbage collection
    .TP
    git\-branchless\-hide(1)
    Hide the provided commits from the smartlog
    .TP
    git\-branchless\-init(1)
    Initialize the branchless workflow for this repository
    .TP
    git\-branchless\-install\-man\-pages(1)
    Install git\-branchless\*(Aqs man\-pages to the given path
    .TP
    git\-branchless\-move(1)
    Move a subtree of commits from one location to another
    .TP
    git\-branchless\-next(1)
    Move to a later commit in the current stack
    .TP
    git\-branchless\-prev(1)
    Move to an earlier commit in the current stack
    .TP
    git\-branchless\-query(1)
    Query the commit graph using the "revset" language and print matching commits
    .TP
    git\-branchless\-repair(1)
    Restore internal invariants by reconciling the internal operation log with the state of the Git repository
    .TP
    git\-branchless\-restack(1)
    Fix up commits abandoned by a previous rewrite operation
    .TP
    git\-branchless\-record(1)
    Create a commit by interactively selecting which changes to include
    .TP
    git\-branchless\-reword(1)
    Reword commits
    .TP
    git\-branchless\-smartlog(1)
    `smartlog` command
    .TP
    git\-branchless\-submit(1)
    Push commits to a remote
    .TP
    git\-branchless\-switch(1)
    Switch to the provided branch or commit
    .TP
    git\-branchless\-sync(1)
    Move any local commit stacks on top of the main branch
    .TP
    git\-branchless\-test(1)
    Run a command on each commit in a given set and aggregate the results
    .TP
    git\-branchless\-undo(1)
    Browse or return to a previous state of the repository
    .TP
    git\-branchless\-unhide(1)
    Unhide previously\-hidden commits from the smartlog
    .TP
    git\-branchless\-wrap(1)
    Wrap a Git command inside a branchless transaction
    .TP
    git\-branchless\-help(1)
    Print this message or the help of the given subcommand(s)
    .SH VERSION
    v0.10.0
    .SH AUTHORS
    Waleed Khan <me@waleedkhan.name>
    "###);
    Ok(())
}
