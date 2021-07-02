//! Tools for interfacing with the Git repository.

mod repo;
mod run;

pub use repo::{wrap_git_error, GitVersion, Repo};
pub use run::GitRunInfo;
