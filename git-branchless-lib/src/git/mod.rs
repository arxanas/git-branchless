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
mod test;
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
pub use test::{
    get_test_locks_dir, get_test_tree_dir, get_test_worktrees_dir, make_test_command_slug,
    SerializedNonZeroOid, SerializedTestResult,
};
pub use tree::{dehydrate_tree, hydrate_tree, Tree};
