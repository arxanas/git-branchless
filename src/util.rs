//! Utility functions.

use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::ffi::OsString;
use std::io::{stderr, stdout, Write};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::str::FromStr;

use anyhow::Context;
use fn_error_context::context;
use git2::ErrorCode;
use log::warn;

use crate::core::config::get_main_branch_name;
use crate::core::eventlog::{EventTransactionId, BRANCHLESS_TRANSACTION_ID_ENV_VAR};
use crate::core::repo::Repo;

/// Convert a `git2::Error` into an `anyhow::Error` with an auto-generated message.
pub fn wrap_git_error(error: git2::Error) -> anyhow::Error {
    anyhow::anyhow!("Git error {:?}: {}", error.code(), error.message())
}

/// Get a mapping from OID to the names of branches which point to that OID.
///
/// The returned branch names do not include the `refs/heads/` prefix.
#[context("Getting branch-OID-to-names map for repository")]
pub fn get_branch_oid_to_names(repo: &Repo) -> anyhow::Result<HashMap<git2::Oid, HashSet<String>>> {
    let branches = repo
        .branches(Some(git2::BranchType::Local))
        .with_context(|| "Reading branches")?;

    let mut result = HashMap::new();
    for branch_info in branches {
        let branch_info = branch_info.with_context(|| "Iterating over branches")?;
        let branch = match branch_info {
            (branch, git2::BranchType::Remote) => anyhow::bail!(
                "Unexpectedly got a remote branch in local branch iterator: {:?}",
                branch.name()
            ),
            (branch, git2::BranchType::Local) => branch,
        };

        let reference = branch.into_reference();
        let reference_name = match reference.shorthand() {
            None => {
                warn!(
                    "Could not decode branch name, skipping: {:?}",
                    reference.name_bytes()
                );
                continue;
            }
            Some(reference_name) => reference_name,
        };

        let branch_oid = reference
            .resolve()
            .with_context(|| format!("Resolving branch into commit: {}", reference_name))?
            .target()
            .unwrap();
        result
            .entry(branch_oid)
            .or_insert_with(HashSet::new)
            .insert(reference_name.to_owned());
    }

    // The main branch may be a remote branch, in which case it won't be
    // returned in the iteration above.
    let main_branch_name = get_main_branch_name(repo)?;
    let main_branch_oid = repo.get_main_branch_oid()?;
    result
        .entry(main_branch_oid)
        .or_insert_with(HashSet::new)
        .insert(main_branch_name);

    Ok(result)
}

/// Get the connection to the SQLite database for this repository.
#[context("Getting connection to SQLite database for repo")]
pub fn get_db_conn(repo: &git2::Repository) -> anyhow::Result<rusqlite::Connection> {
    let dir = repo.path().join("branchless");
    std::fs::create_dir_all(&dir).with_context(|| "Creating .git/branchless dir")?;
    let path = dir.join("db.sqlite3");
    let conn = rusqlite::Connection::open(&path)
        .with_context(|| format!("Opening database connection at {:?}", &path))?;
    Ok(conn)
}

/// Path to the `git` executable on disk to be executed.
#[derive(Clone, Debug)]
pub struct GitRunInfo {
    /// The path to the Git executable on disk.
    pub path_to_git: PathBuf,

    /// The working directory that the Git executable should be run in.
    pub working_directory: PathBuf,

    /// The environment variables that should be passed to the Git process.
    pub env: HashMap<OsString, OsString>,
}

/// Run Git in a subprocess, and inform the user.
///
/// This is suitable for commands which affect the working copy or should run
/// hooks. We don't want our process to be responsible for that.
///
/// `args` contains the list of arguments to pass to Git, not including the Git
/// executable itself.
///
/// Returns the exit code of Git (non-zero signifies error).
#[context("Running Git ({:?}) with args: {:?}", git_run_info, args)]
#[must_use = "The return code for `run_git` must be checked"]
pub fn run_git<S: AsRef<str> + std::fmt::Debug>(
    git_run_info: &GitRunInfo,
    event_tx_id: Option<EventTransactionId>,
    args: &[S],
) -> anyhow::Result<isize> {
    let GitRunInfo {
        path_to_git,
        working_directory,
        env,
    } = git_run_info;
    println!(
        "branchless: {} {}",
        path_to_git.to_string_lossy(),
        args.iter()
            .map(|arg| arg.as_ref())
            .collect::<Vec<_>>()
            .join(" ")
    );
    stdout().flush()?;
    stderr().flush()?;

    let mut command = Command::new(path_to_git);
    command.current_dir(working_directory);
    command.args(args.iter().map(|arg| arg.as_ref()));
    command.env_clear();
    command.envs(env.iter());
    if let Some(event_tx_id) = event_tx_id {
        command.env(BRANCHLESS_TRANSACTION_ID_ENV_VAR, event_tx_id.to_string());
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("Spawning Git subrocess: {:?} {:?}", path_to_git, args))?;
    let exit_status = child.wait().with_context(|| {
        format!(
            "Waiting for Git subprocess to complete: {:?} {:?}",
            path_to_git, args
        )
    })?;

    // On Unix, if the child process was terminated by a signal, we need to call
    // some Unix-specific functions to access the signal that terminated it. For
    // simplicity, just return `1` in those cases.
    let exit_code = exit_status.code().unwrap_or(1);
    let exit_code = exit_code
        .try_into()
        .with_context(|| format!("Converting exit code {} from i32 to isize", exit_code))?;
    Ok(exit_code)
}

/// Run Git silently (don't display output to the user).
///
/// Whenever possible, use `git2`'s bindings to Git instead, as they're
/// considerably more lightweight and reliable.
///
/// Returns the stdout of the Git invocation.
pub fn run_git_silent<S: AsRef<str> + std::fmt::Debug>(
    repo: &git2::Repository,
    git_run_info: &GitRunInfo,
    event_tx_id: Option<EventTransactionId>,
    args: &[S],
) -> anyhow::Result<String> {
    let GitRunInfo {
        path_to_git,
        working_directory,
        env,
    } = git_run_info;

    // Technically speaking, we should be able to work with non-UTF-8 repository
    // paths. Need to make the typechecker accept it.
    let repo_path = repo.path();
    let repo_path = repo_path.to_str().ok_or_else(|| {
        anyhow::anyhow!(
            "Path to Git repo could not be converted to UTF-8 string: {:?}",
            repo_path
        )
    })?;

    let args = {
        let mut result = vec!["-C", repo_path];
        result.extend(args.iter().map(|arg| arg.as_ref()));
        result
    };
    let mut command = Command::new(path_to_git);
    command.args(&args);
    command.current_dir(working_directory);
    command.env_clear();
    command.envs(env.iter());
    if let Some(event_tx_id) = event_tx_id {
        command.env(BRANCHLESS_TRANSACTION_ID_ENV_VAR, event_tx_id.to_string());
    }
    let result = command
        .output()
        .with_context(|| format!("Spawning Git subprocess: {:?} {:?}", path_to_git, args))?;
    let result = String::from_utf8(result.stdout).with_context(|| {
        format!(
            "Decoding stdout from Git subprocess: {:?} {:?}",
            path_to_git, args
        )
    })?;
    Ok(result)
}

/// Run a provided Git hook if it exists for the repository.
///
/// See the man page for `githooks(5)` for more detail on Git hooks.
#[context("Running Git hook: {}", hook_name)]
pub fn run_hook(
    git_run_info: &GitRunInfo,
    repo: &git2::Repository,
    hook_name: &str,
    event_tx_id: EventTransactionId,
    args: &[impl AsRef<str>],
    stdin: Option<String>,
) -> anyhow::Result<()> {
    let hook_dir = repo
        .config()?
        .get_path("core.hooksPath")
        .unwrap_or_else(|_| repo.path().join("hooks"));

    let GitRunInfo {
        // We're calling a Git hook, but not Git itself.
        path_to_git: _,
        // We always want to call the hook in the Git working copy,
        // regardless of where the Git executable was invoked.
        working_directory: _,
        env,
    } = git_run_info;
    let path = {
        let mut path_components: Vec<PathBuf> = vec![std::fs::canonicalize(&hook_dir)?];
        if let Some(path) = env.get(&OsString::from("PATH")) {
            path_components.extend(std::env::split_paths(path));
        }
        std::env::join_paths(path_components)?
    };

    if hook_dir.join(hook_name).exists() {
        let mut child = Command::new(get_sh().context("shell needed to run hook")?)
            // From `githooks(5)`: Before Git invokes a hook, it changes its
            // working directory to either $GIT_DIR in a bare repository or the
            // root of the working tree in a non-bare repository.
            .current_dir(repo.workdir().unwrap_or_else(|| repo.path()))
            .arg("-c")
            .arg(format!("{} \"$@\"", hook_name))
            .arg(hook_name) // "$@" expands "$1" "$2" "$3" ... but we also must specify $0.
            .args(args.iter().map(|arg| arg.as_ref()))
            .env_clear()
            .envs(env.iter())
            .env(BRANCHLESS_TRANSACTION_ID_ENV_VAR, event_tx_id.to_string())
            .env("PATH", &path)
            .stdin(Stdio::piped())
            .spawn()
            .with_context(|| format!("Invoking {} hook with PATH: {:?}", &hook_name, &path))?;

        if let Some(stdin) = stdin {
            write!(child.stdin.as_mut().unwrap(), "{}", stdin)
                .with_context(|| "Writing hook process stdin")?;
        }

        let _ignored: ExitStatus = child.wait()?;
    }
    Ok(())
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

/// Get the reference corresponding to `HEAD`. Don't use
/// `git2::Repository::head` because that resolves the reference before
/// returning it.
pub fn get_repo_head(repo: &git2::Repository) -> anyhow::Result<git2::Reference> {
    repo.find_reference("HEAD").map_err(wrap_git_error)
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
