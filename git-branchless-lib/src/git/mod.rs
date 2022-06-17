//! Tools for interfacing with the Git repository.

mod config;
mod diff;
mod index;
mod oid;
mod repo;
mod run;
mod snapshot;
mod status;
mod tree;

pub use config::{Config, ConfigRead, ConfigValue, ConfigWrite};
pub use diff::{process_diff_for_record, Diff};
pub use index::{update_index, Index, IndexEntry, Stage, UpdateIndexCommand};
pub use oid::{MaybeZeroOid, NonZeroOid};
pub use repo::{
    message_prettify, AmendFastOptions, Branch, BranchType, CategorizedReferenceName,
    CherryPickFastError, CherryPickFastOptions, Commit, GitVersion, PatchId, Reference,
    ReferenceTarget, Repo, ResolvedReferenceInfo, Time,
};
pub use run::{GitRunInfo, GitRunOpts, GitRunResult};
pub use snapshot::WorkingCopySnapshot;
pub use status::{FileMode, FileStatus, StatusEntry};
pub use tree::{dehydrate_tree, hydrate_tree, Tree};
