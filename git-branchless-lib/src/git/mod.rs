//! Tools for interfacing with the Git repository.

mod config;
mod oid;
mod repo;
mod run;
mod snapshot;
mod status;
mod tree;

pub use config::{Config, ConfigRead, ConfigValue, ConfigWrite};
pub use oid::{MaybeZeroOid, NonZeroOid};
pub use repo::{
    message_prettify, AmendFastOptions, Branch, CategorizedReferenceName, CherryPickFastError,
    CherryPickFastOptions, Commit, Diff, GitVersion, PatchId, Reference, ReferenceTarget, Repo,
    RepoReferencesSnapshot, ResolvedReferenceInfo,
};
pub use run::{check_out_commit, CheckOutCommitOptions, GitRunInfo};
pub use status::{FileStatus, StatusEntry};
pub use tree::Tree;
