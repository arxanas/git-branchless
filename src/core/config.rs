//! Accesses repo-specific configuration.

use std::path::PathBuf;

use crate::git::{ConfigRead, Repo};

/// Get the path where Git hooks are stored on disk.
pub fn get_core_hooks_path(repo: &Repo) -> eyre::Result<PathBuf> {
    repo.get_readonly_config()?
        .get_or_else("core.hooksPath", || repo.get_path().join("hooks"))
}

/// Get the configured name of the main branch.
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
pub fn get_default_branch_name(repo: &Repo) -> eyre::Result<Option<String>> {
    let config = repo.get_readonly_config()?;
    let default_branch_name: Option<String> = config.get("init.defaultBranch")?;
    Ok(default_branch_name)
}

/// If `true`, when restacking a commit, do not update its timestamp to the
/// current time.
pub fn get_restack_preserve_timestamps(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.restack.preserveTimestamps", false)
}

/// If `true`, when advancing to a "next" commit, prompt interactively to
/// if there is ambiguity in which commit to advance to.
pub fn get_next_interactive(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.next.interactive", false)
}

/// Config key for `get_restack_warn_abandoned`.
pub const RESTACK_WARN_ABANDONED_CONFIG_KEY: &str = "branchless.restack.warnAbandoned";

/// If `true`, when a rewrite event happens which abandons commits, warn the user
/// and tell them to run `git restack`.
pub fn get_restack_warn_abandoned(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or(RESTACK_WARN_ABANDONED_CONFIG_KEY, true)
}

/// If `true`, show branches pointing to each commit in the smartlog.
pub fn get_commit_metadata_branches(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.commitMetadata.branches", true)
}

/// If `true`, show associated Phabricator commits in the smartlog.
pub fn get_commit_metadata_differential_revision(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.commitMetadata.differentialRevision", true)
}

/// If `true`, show the age of each commit in the smartlog.
pub fn get_commit_metadata_relative_time(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.commitMetadata.relativeTime", true)
}
