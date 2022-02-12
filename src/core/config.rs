//! Accesses repo-specific configuration.

use std::path::PathBuf;

use tracing::instrument;

use crate::git::{ConfigRead, Repo};

/// Get the path where Git hooks are stored on disk.
#[instrument]
pub fn get_core_hooks_path(repo: &Repo) -> eyre::Result<PathBuf> {
    repo.get_readonly_config()?
        .get_or_else("core.hooksPath", || repo.get_path().join("hooks"))
}

/// Get the configured name of the main branch.
#[instrument]
pub fn get_main_branch_name(repo: &Repo) -> eyre::Result<String> {
    let config = repo.get_readonly_config()?;
    let main_branch_name: Option<String> = config.get("branchless.core.mainBranch")?;
    let main_branch_name = match main_branch_name {
        Some(main_branch_name) => main_branch_name,
        None => {
            // Deprecated; use `branchless.core.mainBranch` instead.
            config
                .get("branchless.mainBranch")?
                .unwrap_or_else(|| "master".to_string())
        }
    };
    Ok(main_branch_name)
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
    use std::path::{Path, PathBuf};

    use tracing::instrument;

    /// Path to the Git executable to shell out to as a subprocess when
    /// appropriate. This may be set during tests.
    pub const PATH_TO_GIT: &str = "PATH_TO_GIT";

    /// "Path to wherever your core Git programs are installed". You can find
    /// the default value by running `git --exec-path`.
    ///
    /// See <https://git-scm.com/docs/git#Documentation/git.txt---exec-pathltpathgt>.
    pub const GIT_EXEC_PATH: &str = "GIT_EXEC_PATH";

    /// Get the path to the Git executable for testing.
    #[instrument]
    pub fn get_path_to_git() -> eyre::Result<PathBuf> {
        let path_to_git = std::env::var_os(PATH_TO_GIT).ok_or_else(|| {
            eyre::eyre!(
                "No path to Git executable was set. Try running as: {}=$(which git) cargo test ...",
                PATH_TO_GIT,
            )
        })?;
        let path_to_git = PathBuf::from(&path_to_git);
        Ok(path_to_git)
    }

    /// Get the `GIT_EXEC_PATH` environment variable for testing.
    #[instrument]
    pub fn get_git_exec_path(path_to_git: &Path) -> PathBuf {
        match std::env::var_os(GIT_EXEC_PATH) {
            Some(git_exec_path) => git_exec_path.into(),
            None => {
                let git_path = path_to_git
                    .parent()
                    .expect("Unable to find git path parent");
                git_path.to_path_buf()
            }
        }
    }
}
