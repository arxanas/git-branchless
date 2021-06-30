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
use std::path::Path;

use anyhow::Context;
use fn_error_context::context;

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

    /// Get the OID for the repository's `HEAD` reference.
    #[context("Getting HEAD OID for repository at : {:?}", self.repo.path())]
    pub fn get_head_oid(&self) -> anyhow::Result<Option<git2::Oid>> {
        let head_ref = match self.repo.head() {
            Ok(head_ref) => Ok(head_ref),
            Err(err)
                if err.code() == git2::ErrorCode::NotFound
                    || err.code() == git2::ErrorCode::UnbornBranch =>
            {
                return Ok(None)
            }
            Err(err) => Err(err),
        }?;
        let head_commit = head_ref.peel_to_commit()?;
        Ok(Some(head_commit.id()))
    }
}
