//! Tools for interfacing with the Git repository.

mod config;
mod dag;
mod oid;
mod repo;
mod run;

pub use self::dag::Dag;
pub use config::{Config, ConfigValue};
pub use oid::{MaybeZeroOid, NonZeroOid};
pub use repo::{
    Branch, CategorizedReferenceName, Commit, GitVersion, PatchId, Reference, ReferenceTarget, Repo,
};
pub use run::GitRunInfo;
