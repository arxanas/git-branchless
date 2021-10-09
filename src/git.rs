//! Tools for interfacing with the Git repository.

mod config;
mod dag;
mod oid;
mod repo;
mod run;
mod tree;

pub use config::{Config, ConfigRead, ConfigValue, ConfigWrite};
pub use dag::{
    commit_set_to_vec, resolve_commits, sort_commit_set, CommitSet, CommitVertex, Dag,
    ResolveCommitsResult,
};
pub use oid::{MaybeZeroOid, NonZeroOid};
pub use repo::{
    Branch, CategorizedReferenceName, CherryPickFastError, CherryPickFastOptions, Commit, Diff,
    GitVersion, PatchId, Reference, ReferenceTarget, Repo, RepoReferencesSnapshot,
    ResolvedReferenceInfo,
};
pub use run::GitRunInfo;
pub use tree::Tree;
