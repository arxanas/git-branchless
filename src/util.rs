//! Utility functions.

use std::path::PathBuf;
use std::str::FromStr;

use fn_error_context::context;
use git2::ErrorCode;

/// Convert a `git2::Error` into an `anyhow::Error` with an auto-generated message.
pub fn wrap_git_error(error: git2::Error) -> anyhow::Error {
    anyhow::anyhow!("Git error {:?}: {}", error.code(), error.message())
}

/// The parsed version of Git.
#[derive(Debug, PartialEq, PartialOrd, Eq)]
pub struct GitVersion(pub isize, pub isize, pub isize);

impl FromStr for GitVersion {
    type Err = anyhow::Error;

    #[context("Parsing Git version from string: {:?}", output)]
    fn from_str(output: &str) -> anyhow::Result<GitVersion> {
        let output = output.trim();
        let words = output.split(' ').collect::<Vec<&str>>();
        let version_str = match &words.as_slice() {
            [_git, _version, version_str, ..] => version_str,
            _ => anyhow::bail!("Could not parse Git version output: {:?}", output),
        };
        match version_str.split('.').collect::<Vec<&str>>().as_slice() {
            [major, minor, patch, ..] => {
                let major = major.parse()?;
                let minor = minor.parse()?;
                let patch = patch.parse()?;
                Ok(GitVersion(major, minor, patch))
            }
            _ => anyhow::bail!("Could not parse Git version string: {}", version_str),
        }
    }
}

/// Returns a path for a given file, searching through PATH to find it.
pub fn get_from_path(exe_name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let bash_path = dir.join(exe_name);
            if bash_path.is_file() {
                Some(bash_path)
            } else {
                None
            }
        })
    })
}

/// Returns the path to a shell suitable for running hooks.
pub fn get_sh() -> Option<PathBuf> {
    let exe_name = if cfg!(target_os = "windows") {
        "bash.exe"
    } else {
        "sh"
    };
    // If we are on Windows, first look for git.exe, and try to use it's bash, otherwise it won't
    // be able to find git-branchless correctly.
    if cfg!(target_os = "windows") {
        // Git is typically installed at C:\Program Files\Git\cmd\git.exe with the cmd\ directory
        // in the path, however git-bash is usually not in PATH and is in bin\ directory:
        let git_path = get_from_path("git.exe").expect("Couldn't find git.exe");
        let git_dir = git_path.parent().unwrap().parent().unwrap();
        let git_bash = git_dir.join("bin").join(exe_name);
        if git_bash.is_file() {
            return Some(git_bash);
        }
    }
    get_from_path(exe_name)
}

/// The result of attempting to resolve commits.
pub enum ResolveCommitsResult<'repo> {
    /// All commits were successfully resolved.
    Ok {
        /// The commits.
        commits: Vec<git2::Commit<'repo>>,
    },

    /// The first commit which couldn't be resolved.
    CommitNotFound {
        /// The identifier of the commit, as provided by the user.
        commit: String,
    },
}

/// Parse strings which refer to commits, such as:
///
/// - Full OIDs.
/// - Short OIDs.
/// - Reference names.
#[context("Resolving commits")]
pub fn resolve_commits(
    repo: &git2::Repository,
    hashes: Vec<String>,
) -> anyhow::Result<ResolveCommitsResult> {
    let mut commits = Vec::new();
    for hash in hashes {
        let commit = match repo.revparse_single(&hash) {
            Ok(commit) => match commit.into_commit() {
                Ok(commit) => commit,
                Err(_) => return Ok(ResolveCommitsResult::CommitNotFound { commit: hash }),
            },
            Err(err) if err.code() == ErrorCode::NotFound => {
                return Ok(ResolveCommitsResult::CommitNotFound { commit: hash })
            }
            Err(err) => return Err(err.into()),
        };
        commits.push(commit)
    }
    Ok(ResolveCommitsResult::Ok { commits })
}

#[cfg(test)]
mod tests {
    use crate::testing::make_git;

    use super::*;

    #[test]
    fn test_parse_git_version_output() {
        assert_eq!(
            "git version 12.34.56".parse::<GitVersion>().unwrap(),
            GitVersion(12, 34, 56)
        );
        assert_eq!(
            "git version 12.34.56\n".parse::<GitVersion>().unwrap(),
            GitVersion(12, 34, 56)
        );
        assert_eq!(
            "git version 12.34.56.78.abcdef"
                .parse::<GitVersion>()
                .unwrap(),
            GitVersion(12, 34, 56)
        );
    }

    #[test]
    fn test_hook_working_dir() -> anyhow::Result<()> {
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
                   echo "Contents of test1.txt:"
                   cat test1.txt
                   "#,
        )?;

        {
            // Trigger the `post-rewrite` hook that we wrote above.
            let (stdout, stderr) = git.run(&["commit", "--amend", "-m", "foo"])?;
            insta::assert_snapshot!(stderr, @r###"
                branchless: processing 2 updates to branches/refs
                branchless: processing commit
                Contents of test1.txt:
                test1 contents
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
}
