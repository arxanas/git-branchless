//! Operations on the Git repository. This module exists for a few reasons:
//!
//! - To ensure that every call to a Git operation has an associated `context`
//! for use with `Try`.
//! - To improve the interface in some cases. In particular, some operations in
//! `git2` return an `Error` with code `ENOTFOUND`, but we should really return
//! an `Option` in those cases.
//! - To make it possible to audit all the Git operations carried out in the
//! codebase.
//! - To collect some different helper Git functions.

use std::borrow::{Borrow, BorrowMut};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Context;
use fn_error_context::context;

use crate::core::config::get_main_branch_name;
use crate::util::wrap_git_error;

/// Wrapper around `git2::Repository`.
pub struct Repo {
    repo: git2::Repository,
}

impl std::ops::Deref for Repo {
    type Target = git2::Repository;

    fn deref(&self) -> &Self::Target {
        &self.repo
    }
}

impl std::ops::DerefMut for Repo {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.repo
    }
}

impl Borrow<git2::Repository> for Repo {
    fn borrow(&self) -> &git2::Repository {
        &self.repo
    }
}

impl BorrowMut<git2::Repository> for Repo {
    fn borrow_mut(&mut self) -> &mut git2::Repository {
        &mut self.repo
    }
}

impl From<git2::Repository> for Repo {
    fn from(repo: git2::Repository) -> Self {
        Repo { repo }
    }
}

/// Information about the current `HEAD` of the repository.
///
/// `HEAD` is typically a symbolic reference, which means that it's a reference
/// that points to another reference. Usually, the other reference is a branch.
/// In this way, you can check out a branch and move the branch (e.g. by
/// committing) and `HEAD` is also effectively updated (you can traverse the
/// pointed-to reference and get the current commit OID).
///
/// There are a couple of interesting edge cases to worry about:
///
/// - `HEAD` is detached. This means that it's pointing directly to a commit and
/// is not a symbolic reference for the time being. This is uncommon in normal
/// Git usage, but very common in `git-branchless` usage.
/// - `HEAD` is unborn. This means that it doesn't even exist yet. This happens
/// when a repository has been freshly initialized, but no commits have been
/// made, for example.
pub struct HeadInfo {
    /// The OID of the commit that `HEAD` points to. If `HEAD` is unborn, then
    /// this is `None`.
    pub oid: Option<git2::Oid>,

    /// The name of the reference that `HEAD` points to symbolically. If `HEAD`
    /// is detached, then this is `None`.
    reference_name: Option<String>,
}

impl HeadInfo {
    /// Get the name of the branch, if any. Returns `None` if `HEAD` is
    /// detached.  The `refs/heads/` prefix, if any, is stripped.
    pub fn branch_name(&self) -> Option<&str> {
        self.reference_name
            .as_ref()
            .map(|name| match name.strip_prefix("refs/heads/") {
                Some(branch_name) => branch_name,
                None => &name,
            })
    }
}

impl Repo {
    /// Get the Git repository associated with the given directory.
    #[context("Getting Git repository for directory: {:?}", &path)]
    pub fn from_dir(path: &Path) -> anyhow::Result<Self> {
        let repository = git2::Repository::discover(path).map_err(wrap_git_error)?;
        Ok(repository.into())
    }

    /// Get the Git repository associated with the current directory.
    #[context("Getting Git repository for current directory")]
    pub fn from_current_dir() -> anyhow::Result<Self> {
        let path = std::env::current_dir().with_context(|| "Getting working directory")?;
        Repo::from_dir(&path)
    }

    /// Get the connection to the SQLite database for this repository.
    #[context("Getting connection to SQLite database for repo")]
    pub fn get_db_conn(&self) -> anyhow::Result<rusqlite::Connection> {
        let dir = self.repo.path().join("branchless");
        std::fs::create_dir_all(&dir).with_context(|| "Creating .git/branchless dir")?;
        let path = dir.join("db.sqlite3");
        let conn = rusqlite::Connection::open(&path)
            .with_context(|| format!("Opening database connection at {:?}", &path))?;
        Ok(conn)
    }

    /// Get the OID for the repository's `HEAD` reference.
    #[context("Getting `HEAD` info for repository at: {:?}", self.repo.path())]
    pub fn get_head_info(&self) -> anyhow::Result<HeadInfo> {
        let head_reference = match self.repo.find_reference("HEAD") {
            Err(err) if err.code() == git2::ErrorCode::NotFound => None,
            Err(err) => return Err(wrap_git_error(err)),
            Ok(result) => Some(result),
        };
        let (head_oid, reference_name) = match &head_reference {
            Some(head_reference) => {
                let head_oid = head_reference
                    .peel_to_commit()
                    .with_context(|| "Resolving `HEAD` reference")?
                    .id();
                let reference_name = match head_reference.kind() {
                    Some(git2::ReferenceType::Direct) => None,
                    Some(git2::ReferenceType::Symbolic) => match head_reference.symbolic_target() {
                        Some(name) => Some(name.to_string()),
                        None => anyhow::bail!(
                            "`HEAD` reference was resolved to OID: {:?}, but its name could not be decoded: {:?}",
                            head_oid, head_reference.name_bytes()
                        ),
                    }
                    None => anyhow::bail!("Unknown `HEAD` reference type")
                };
                (Some(head_oid), reference_name)
            }
            None => (None, None),
        };
        Ok(HeadInfo {
            oid: head_oid,
            reference_name,
        })
    }

    /// Get the OID corresponding to the main branch.
    #[context("Getting main branch OID for repository at: {:?}", self.repo.path())]
    pub fn get_main_branch_oid(&self) -> anyhow::Result<git2::Oid> {
        let main_branch_name = get_main_branch_name(&self.repo)?;
        let branch = self
            .repo
            .find_branch(&main_branch_name, git2::BranchType::Local)
            .or_else(|_| {
                self.repo
                    .find_branch(&main_branch_name, git2::BranchType::Remote)
            });
        let branch = match branch {
            Ok(branch) => branch,
            // Drop the error trace here. It's confusing, and we don't want it to appear in the output.
            Err(_) => anyhow::bail!(
                r"
The main branch {:?} could not be found in your repository.
Either create it, or update the main branch setting by running:

    git config branchless.core.mainBranch <branch>
",
                main_branch_name
            ),
        };
        let commit = branch.get().peel_to_commit()?;
        Ok(commit.id())
    }

    /// Get a mapping from OID to the names of branches which point to that OID.
    ///
    /// The returned branch names do not include the `refs/heads/` prefix.
    #[context("Getting branch-OID-to-names map for repository at: {:?}", self.repo.path())]
    pub fn get_branch_oid_to_names(&self) -> anyhow::Result<HashMap<git2::Oid, HashSet<String>>> {
        let branches = self
            .repo
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
                    log::warn!(
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
        let main_branch_name = get_main_branch_name(&self.repo)?;
        let main_branch_oid = self.get_main_branch_oid()?;
        result
            .entry(main_branch_oid)
            .or_insert_with(HashSet::new)
            .insert(main_branch_name);

        Ok(result)
    }
}
