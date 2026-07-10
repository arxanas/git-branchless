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
pub use diff::{Diff, process_diff_for_record, summarize_diff_for_temporary_commit};
pub use index::{Index, IndexEntry, Stage, UpdateIndexCommand, update_index};
pub use object::Commit;
pub use oid::{MaybeZeroOid, NonZeroOid};
pub use reference::{
    Branch, BranchType, CategorizedReferenceName, Reference, ReferenceName, ReferenceTarget,
};
pub use repo::{
    AmendFastOptions, CherryPickFastOptions, CreateCommitFastError, Error as RepoError,
    GitErrorCode, GitVersion, PatchId, Repo, ResolvedReferenceInfo, Result as RepoResult,
    Signature, Time, message_prettify,
};
pub use run::{GitRunInfo, GitRunOpts, GitRunResult};
pub use snapshot::{WorkingCopyChangesType, WorkingCopySnapshot};
pub use status::{FileMode, FileStatus, StatusEntry};
pub use test::{
    SerializedNonZeroOid, SerializedTestResult, TEST_ABORT_EXIT_CODE, TEST_INDETERMINATE_EXIT_CODE,
    TEST_SUCCESS_EXIT_CODE, TestCommand, get_latest_test_command_path, get_test_locks_dir,
    get_test_tree_dir, get_test_worktrees_dir, make_test_command_slug,
};
pub use tree::{
    Tree, dehydrate_tree, get_changed_paths_between_trees, hydrate_tree, make_empty_tree,
};
