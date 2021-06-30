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

impl Repo {}
