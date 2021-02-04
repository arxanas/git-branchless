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
            assert!(stdout.replace("\n", " ").contains("requires Git v2.29"));
        }

        Ok(())
    })
}
