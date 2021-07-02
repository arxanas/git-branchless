//! Tools for interfacing with the Git repository.

mod repo;
mod run;

pub use repo::{GitVersion, Repo};
pub use run::GitRunInfo;
