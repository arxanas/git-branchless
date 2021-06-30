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

impl From<git2::Repository> for Repo {
    fn from(repo: git2::Repository) -> Self {
        Repo { repo }
    }
}

impl Repo {
    #[context("Getting Git repository for directory: {:?}", &path)]
    fn from_dir(path: &Path) -> anyhow::Result<Self> {
        let repository = git2::Repository::discover(path).map_err(wrap_git_error)?;
        Ok(repository.into())
    }

    /// Get the Git repository associated with the current directory.
    #[context("Getting Git repository for current directory")]
    pub fn from_current_dir() -> anyhow::Result<Self> {
        let path = std::env::current_dir().with_context(|| "Getting working directory")?;
        Repo::from_dir(&path)
    }
}
