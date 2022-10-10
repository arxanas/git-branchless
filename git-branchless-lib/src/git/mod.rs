//! Tools for interfacing with the Git repository.

mod config;
mod diff;
mod index;
mod object;
mod oid;
mod reference;
mod repo;
mod run;
mod snapshot;
mod status;
mod tree;

pub use config::{Config, ConfigRead, ConfigValue, ConfigWrite};
pub use diff::{process_diff_for_record, Diff};
pub use index::{update_index, Index, IndexEntry, Stage, UpdateIndexCommand};
pub use object::Commit;
pub use oid::{MaybeZeroOid, NonZeroOid};
pub use reference::{
    Branch, BranchType, CategorizedReferenceName, Reference, ReferenceName, ReferenceTarget,
};
pub use repo::{
    message_prettify, AmendFastOptions, CherryPickFastError, CherryPickFastOptions,
    Error as RepoError, GitVersion, PatchId, Repo, ResolvedReferenceInfo, Result as RepoResult,
    Time,
};
pub use run::{GitRunInfo, GitRunOpts, GitRunResult};
pub use snapshot::{WorkingCopyChangesType, WorkingCopySnapshot};
pub use status::{FileMode, FileStatus, StatusEntry};
pub use tree::{dehydrate_tree, hydrate_tree, Tree};
