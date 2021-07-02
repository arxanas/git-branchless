use crate::util::trim_lines;

use anyhow::Context;
use branchless::git::GitVersion;
use branchless::testing::{make_git, GitInitOptions, GitRunOptions};

#[test]
fn test_hook_installed() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    let hook_path = git.repo_path.join(".git").join("hooks").join("post-commit");
    assert!(hook_path.exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(&hook_path)
            .with_context(|| format!("Reading hook permissions for {:?}", &hook_path))?;
        let mode = metadata.permissions().mode();
        assert!(mode & 0o111 == 0o111);
    }

    Ok(())
}

#[test]
fn test_alias_installed() -> anyhow::Result<()> {
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
fn test_old_git_version_warning() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    let version = git.get_version()?;
    if version < GitVersion(2, 29, 0) {
        let (stdout, _stderr) = git.run(&["branchless", "init"])?;
        let (version_str, _stderr) = git.run(&["version"])?;
        let stdout = stdout.replace(version_str.trim(), "<git version output>");
        insta::assert_snapshot!(stdout, @r###"
            Auto-detected your main branch as: master
            If this is incorrect, run: git config branchless.core.mainBranch <branch>
            Setting config (non-global): branchless.core.mainBranch = master
            Setting config (non-global): advice.detachedHead = false
            Installing hook: post-commit
            Installing hook: post-rewrite
            Installing hook: post-checkout
            Installing hook: pre-auto-gc
            Installing hook: reference-transaction
            Installing alias (non-global): git smartlog -> git branchless smartlog
            Installing alias (non-global): git sl -> git branchless smartlog
            Installing alias (non-global): git hide -> git branchless hide
            Installing alias (non-global): git unhide -> git branchless unhide
            Installing alias (non-global): git prev -> git branchless prev
            Installing alias (non-global): git next -> git branchless next
            Installing alias (non-global): git restack -> git branchless restack
            Installing alias (non-global): git undo -> git branchless undo
            Installing alias (non-global): git move -> git branchless move
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

#[test]
fn test_init_basic() -> anyhow::Result<()> {
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
            Auto-detected your main branch as: master
            If this is incorrect, run: git config branchless.core.mainBranch <branch>
            Setting config (non-global): branchless.core.mainBranch = master
            Setting config (non-global): advice.detachedHead = false
            Installing hook: post-commit
            Installing hook: post-rewrite
            Installing hook: post-checkout
            Installing hook: pre-auto-gc
            Installing hook: reference-transaction
            Installing alias (non-global): git smartlog -> git branchless smartlog
            Installing alias (non-global): git sl -> git branchless smartlog
            Installing alias (non-global): git hide -> git branchless hide
            Installing alias (non-global): git unhide -> git branchless unhide
            Installing alias (non-global): git prev -> git branchless prev
            Installing alias (non-global): git next -> git branchless next
            Installing alias (non-global): git restack -> git branchless restack
            Installing alias (non-global): git undo -> git branchless undo
            Installing alias (non-global): git move -> git branchless move
            Successfully installed git-branchless.
            To uninstall, run: git branchless init --uninstall
            "###);
    }

    Ok(())
}

#[test]
fn test_init_prompt_for_main_branch() -> anyhow::Result<()> {
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
            Your main branch name could not be auto-detected!
            Examples of a main branch: master, main, trunk, etc.
            See https://github.com/arxanas/git-branchless/wiki/Concepts#main-branch
            Enter the name of your main branch: Setting config (non-global): branchless.core.mainBranch = bespoke
            Setting config (non-global): advice.detachedHead = false
            Installing hook: post-commit
            Installing hook: post-rewrite
            Installing hook: post-checkout
            Installing hook: pre-auto-gc
            Installing hook: reference-transaction
            Installing alias (non-global): git smartlog -> git branchless smartlog
            Installing alias (non-global): git sl -> git branchless smartlog
            Installing alias (non-global): git hide -> git branchless hide
            Installing alias (non-global): git unhide -> git branchless unhide
            Installing alias (non-global): git prev -> git branchless prev
            Installing alias (non-global): git next -> git branchless next
            Installing alias (non-global): git restack -> git branchless restack
            Installing alias (non-global): git undo -> git branchless undo
            Installing alias (non-global): git move -> git branchless move
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
fn test_main_branch_not_found_error_message() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.run(&["branch", "-d", "master"])?;

    {
        let (stdout, stderr) = git.run_with_options(
            &["smartlog"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(trim_lines(stderr), @r###"
        Error: Getting main branch OID for repository at: "<repo-path>/.git/"

        Caused by:

            The main branch "master" could not be found in your repository.
            Either create it, or update the main branch setting by running:

                git config branchless.core.mainBranch <branch>

        "###);
        insta::assert_snapshot!(stdout, @"");
    }

    Ok(())
}

#[test]
fn test_init_uninstall() -> anyhow::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    {
        let (stdout, stderr) = git.run(&["branchless", "init", "--uninstall"])?;
        insta::assert_snapshot!(stderr, @"");
        insta::assert_snapshot!(stdout, @r###"
            Unsetting config (non-global): branchless.core.mainBranch
            Unsetting config (non-global): advice.detachedHead
            Uninstalling hook: post-commit
            Uninstalling hook: post-rewrite
            Uninstalling hook: post-checkout
            Uninstalling hook: pre-auto-gc
            Uninstalling hook: reference-transaction
            Uninstalling alias (non-global): git smartlog
            Uninstalling alias (non-global): git sl
            Uninstalling alias (non-global): git hide
            Uninstalling alias (non-global): git unhide
            Uninstalling alias (non-global): git prev
            Uninstalling alias (non-global): git next
            Uninstalling alias (non-global): git restack
            Uninstalling alias (non-global): git undo
            Uninstalling alias (non-global): git move
            "###);
    }

    Ok(())
}
