//! Accesses repo-specific configuration.

use std::path::PathBuf;

use fn_error_context::context;

#[context("Getting repo configuration")]
fn get_config(repo: &git2::Repository) -> anyhow::Result<git2::Config> {
    let result = repo.config()?;
    Ok(result)
}

/// Get the path where Git hooks are stored on disk.
pub fn get_core_hooks_path(repo: &git2::Repository) -> anyhow::Result<PathBuf> {
    let result = get_config(repo)?
        .get_path("core.hooksPath")
        .unwrap_or_else(|_err| repo.path().join("hooks"));
    Ok(result)
}

/// Get the name of the main branch for the repository.
///
/// Args:
/// * `repo`: The Git repository.
///
/// Returns: The name of the main branch for the repository.
pub fn get_main_branch_name(repo: &git2::Repository) -> anyhow::Result<String> {
    get_config(repo)?
        .get_string("branchless.mainBranch")
        .or_else(|_| Ok(String::from("master")))
}

/// If `true`, when restacking a commit, do not update its timestamp to the
/// current time.
///
/// TODO: document this configuration option in the wiki at
/// https://github.com/arxanas/git-branchless/wiki/Configuration
pub fn get_restack_preserve_timestamps(repo: &git2::Repository) -> anyhow::Result<bool> {
    get_config(repo)?
        .get_bool("branchless.restack.preserveTimestamps")
        .or(Ok(false))
}

/// Config key for `get_restack_warn_abandoned`.
pub const RESTACK_WARN_ABANDONED_CONFIG_KEY: &str = "branchless.restack.warnAbandoned";

/// If `true`, when a rewrite event happens which abandons commits, warn the user
/// and tell them to run `git restack`.
pub fn get_restack_warn_abandoned(repo: &git2::Repository) -> anyhow::Result<bool> {
    get_config(repo)?
        .get_bool(RESTACK_WARN_ABANDONED_CONFIG_KEY)
        .or(Ok(true))
}

/// If `true`, show branches pointing to each commit in the smartlog.
pub fn get_commit_metadata_branches(repo: &git2::Repository) -> anyhow::Result<bool> {
    get_config(repo)?
        .get_bool("branchless.commitMetadata.branches")
        .or(Ok(true))
}

/// If `true`, show associated Phabricator commits in the smartlog.
pub fn get_commit_metadata_differential_revision(repo: &git2::Repository) -> anyhow::Result<bool> {
    get_config(repo)?
        .get_bool("branchless.commitMetadata.differentialRevision")
        .or(Ok(true))
}

/// If `true`, show the age of each commit in the smartlog.
pub fn get_commit_metadata_relative_time(repo: &git2::Repository) -> anyhow::Result<bool> {
    get_config(repo)?
        .get_bool("branchless.commitMetadata.relativeTime")
        .or(Ok(true))
}
