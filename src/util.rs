use std::path::Path;

use crate::config::get_main_branch_name;

pub fn wrap_git_error(error: git2::Error) -> anyhow::Error {
    anyhow::anyhow!("Git error {:?}: {}", error.code(), error.message())
}

/// Get the OID for the repository's `HEAD` reference.
///
/// Args:
/// * `repo`: The Git repository.
///
/// Returns: The OID for the repository's `HEAD` reference.
pub fn get_head_oid(repo: &git2::Repository) -> anyhow::Result<git2::Oid> {
    let head_ref = repo.head()?;
    let head_commit = head_ref.peel_to_commit()?;
    Ok(head_commit.id())
}

/// Get the OID corresponding to the main branch.
///
/// Args:
/// * `repo`: The Git repository.
///
/// Returns: The OID corresponding to the main branch.
pub fn get_main_branch_oid(repo: &git2::Repository) -> anyhow::Result<git2::Oid> {
    let main_branch_name = get_main_branch_name(&repo)?;
    let branch = repo
        .find_branch(&main_branch_name, git2::BranchType::Local)
        .or_else(|_| repo.find_branch(&main_branch_name, git2::BranchType::Remote))?;
    let commit = branch.get().peel_to_commit()?;
    Ok(commit.id())
}

/// Get the git repository associated with the current directory.
pub fn get_repo() -> anyhow::Result<git2::Repository> {
    let path = std::env::current_dir()?;
    let repository = git2::Repository::discover(path).map_err(wrap_git_error)?;
    Ok(repository)
}

pub struct GitExecutable<'path>(pub &'path Path);
