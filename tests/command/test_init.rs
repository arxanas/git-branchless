use anyhow::Context;
use branchless::util::GitVersion;

#[cfg(test)]
#[test]
fn test_hook_installed() -> anyhow::Result<()> {
    branchless::testing::with_git(|git| {
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
    })
}

#[test]
fn test_alias_installed() -> anyhow::Result<()> {
    branchless::testing::with_git(|git| {
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
    })
}

#[test]
fn test_old_git_version_warning() -> anyhow::Result<()> {
    branchless::testing::with_git(|git| {
        git.init_repo()?;
        let version = git.get_version()?;
        if version < GitVersion(2, 29, 0) {
            let (stdout, _stderr) = git.run(&["branchless", "init"])?;
            let (version_str, _stderr) = git.run(&["version"])?;
            let stdout = stdout.replace(version_str.trim(), "<git version output>");
            insta::assert_snapshot!(stdout, @r###"
            Installing hook: post-commit
            Installing hook: post-rewrite
            Installing hook: post-checkout
            Installing hook: pre-auto-gc
            Installing hook: reference-transaction
            Setting config (non-global): advice.detachedHead = false
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
            "###);
        }

        Ok(())
    })
}
