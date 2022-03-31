//! Accesses repo-specific configuration.

use std::path::PathBuf;

use tracing::{instrument, warn};

use crate::git::{ConfigRead, Repo};

/// Get the path where Git hooks are stored on disk.
#[instrument]
pub fn get_core_hooks_path(repo: &Repo) -> eyre::Result<PathBuf> {
    repo.get_readonly_config()?
        .get_or_else("core.hooksPath", || repo.get_path().join("hooks"))
}

/// Get the configured name of the main branch.
///
/// The following config values are resolved, in order. The first valid value is returned.
/// - branchless.core.mainBranch
/// - (deprecated) branchless.mainBranch
/// - init.defaultBranch
/// - finally, default to "master"
#[instrument]
pub fn get_main_branch_name(repo: &Repo) -> eyre::Result<String> {
    let config = repo.get_readonly_config()?;

    if let Some(branch_name) = config.get("branchless.core.mainBranch")? {
        return Ok(branch_name);
    }

    if let Some(branch_name) = config.get("branchless.mainBranch")? {
        return Ok(branch_name);
    }

    if let Some(branch_name) = get_default_branch_name(repo)? {
        return Ok(branch_name);
    }

    Ok("master".to_string())
}

/// Get the default comment character.
#[instrument]
pub fn get_comment_char(repo: &Repo) -> eyre::Result<char> {
    let from_config: Option<String> = repo.get_readonly_config()?.get("core.commentChar")?;
    let comment_char = match from_config {
        // Note that git also allows `core.commentChar="auto"`, which we do not currently support.
        Some(comment_char) => comment_char.chars().next().unwrap(),
        None => char::from(git2::DEFAULT_COMMENT_CHAR.unwrap()),
    };
    Ok(comment_char)
}

/// Get the commit template message, if any.
#[instrument]
pub fn get_commit_template(repo: &Repo) -> eyre::Result<Option<String>> {
    let commit_template_path: Option<String> =
        repo.get_readonly_config()?.get("commit.template")?;
    let commit_template_path = match commit_template_path {
        Some(commit_template_path) => PathBuf::from(commit_template_path),
        None => return Ok(None),
    };

    let commit_template_path = if commit_template_path.is_relative() {
        match repo.get_working_copy_path() {
            Some(root) => root.join(commit_template_path),
            None => {
                warn!(?commit_template_path, "Commit template path was relative, but this repository does not have a working copy");
                return Ok(None);
            }
        }
    } else {
        commit_template_path
    };

    match std::fs::read_to_string(&commit_template_path) {
        Ok(contents) => Ok(Some(contents)),
        Err(e) => {
            warn!(?e, ?commit_template_path, "Could not read commit template");
            Ok(None)
        }
    }
}

/// Get the default init branch name.
#[instrument]
pub fn get_default_branch_name(repo: &Repo) -> eyre::Result<Option<String>> {
    let config = repo.get_readonly_config()?;
    let default_branch_name: Option<String> = config.get("init.defaultBranch")?;
    Ok(default_branch_name)
}

/// If `true`, when restacking a commit, do not update its timestamp to the
/// current time.
#[instrument]
pub fn get_restack_preserve_timestamps(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.restack.preserveTimestamps", false)
}

/// If `true`, when advancing to a "next" commit, prompt interactively to
/// if there is ambiguity in which commit to advance to.
#[instrument]
pub fn get_next_interactive(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.next.interactive", false)
}

/// Config key for `get_restack_warn_abandoned`.
pub const RESTACK_WARN_ABANDONED_CONFIG_KEY: &str = "branchless.restack.warnAbandoned";

/// If `true`, when a rewrite event happens which abandons commits, warn the user
/// and tell them to run `git restack`.
#[instrument]
pub fn get_restack_warn_abandoned(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or(RESTACK_WARN_ABANDONED_CONFIG_KEY, true)
}

/// If `true`, show branches pointing to each commit in the smartlog.
#[instrument]
pub fn get_commit_descriptors_branches(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.commitDescriptors.branches", true)
}

/// If `true`, show associated Phabricator commits in the smartlog.
#[instrument]
pub fn get_commit_descriptors_differential_revision(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.commitDescriptors.differentialRevision", true)
}

/// If `true`, show the age of each commit in the smartlog.
#[instrument]
pub fn get_commit_descriptors_relative_time(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.commitDescriptors.relativeTime", true)
}

/// Environment variables which affect the functioning of `git-branchless`.
pub mod env_vars {
    use std::path::PathBuf;

    use tracing::instrument;

    /// Path to the Git executable to shell out to as a subprocess when
    /// appropriate. This may be set during tests.
    pub const TEST_GIT: &str = "TEST_GIT";

    /// "Path to wherever your core Git programs are installed". You can find
    /// the default value by running `git --exec-path`.
    ///
    /// See <https://git-scm.com/docs/git#Documentation/git.txt---exec-pathltpathgt>.
    pub const TEST_GIT_EXEC_PATH: &str = "TEST_GIT_EXEC_PATH";

    /// Get the path to the Git executable for testing.
    #[instrument]
    pub fn get_path_to_git() -> eyre::Result<PathBuf> {
        let path_to_git = std::env::var_os(TEST_GIT).ok_or_else(|| {
            eyre::eyre!(
                "No path to Git executable was set. \
Try running as: `{0}=$(which git) cargo test ...` \
or set `env.{0}` in your `config.toml` \
(see https://doc.rust-lang.org/cargo/reference/config.html)",
                TEST_GIT,
            )
        })?;
        let path_to_git = PathBuf::from(&path_to_git);
        Ok(path_to_git)
    }

    /// Get the `GIT_EXEC_PATH` environment variable for testing.
    #[instrument]
    pub fn get_git_exec_path() -> eyre::Result<PathBuf> {
        let git_exec_path = std::env::var_os(TEST_GIT_EXEC_PATH).ok_or_else(|| {
            eyre::eyre!(
                "No Git exec path was set. \
Try running as: `{0}=$(git --exec-path) cargo test ...` \
or set `env.{0}` in your `config.toml` \
(see https://doc.rust-lang.org/cargo/reference/config.html)",
                TEST_GIT_EXEC_PATH,
            )
        })?;
        let git_exec_path = PathBuf::from(&git_exec_path);
        Ok(git_exec_path)
    }
}
