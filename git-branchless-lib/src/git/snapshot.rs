//! Implementation of working copy snapshots. The ideas are based off of those
//! in Jujutsu: <https://github.com/martinvonz/jj/blob/main/docs/working-copy.md>
//!
//! Normally, Git only tracks committed changes via commits, and a subset of
//! information about uncommitted changes via the index. This module implements
//! "working copy snapshots", which are enough to reproduce the entire tracked
//! contents of the working copy, including staged changes and files with merge
//! conflicts.
//!
//! Untracked changes are not handled by this module. The changes might contain
//! sensitive data which we don't want to accidentally store in Git, or might be
//! very large and cause performance issues if committed.
//!
//! There are two main reasons to implement working copy snapshots:
//!
//!  1. To support enhanced undo features. For example, you should be able to
//!     jump back into merge conflict resolution which was happening at some
//!     past time.
//!  2. To unify the implementations of operations across commits and the
//!     working copy. For example, a `git split` command which splits one commit
//!     into multiple could also be used to split the working copy into multiple
//!     commits.

use itertools::Itertools;
use std::collections::HashMap;
use std::str::FromStr;

use tracing::instrument;

use crate::core::formatting::Pluralize;
use crate::git::FileStatus;

use super::index::{Index, IndexEntry, Stage};
use super::repo::Signature;
use super::status::FileMode;
use super::tree::{hydrate_tree, make_empty_tree};
use super::{
    Commit, MaybeZeroOid, NonZeroOid, ReferenceName, Repo, ResolvedReferenceInfo, StatusEntry,
};

const BRANCHLESS_HEAD_TRAILER: &str = "Branchless-head";
const BRANCHLESS_HEAD_REF_TRAILER: &str = "Branchless-head-ref";
const BRANCHLESS_UNSTAGED_TRAILER: &str = "Branchless-unstaged";

/// A special `Commit` which represents the status of the working copy at a
/// given point in time. This means that it can include changes in any stage.
#[derive(Clone, Debug)]
pub struct WorkingCopySnapshot<'repo> {
    /// The commit which contains the metadata about the `HEAD` commit and all
    /// the "stage commits" included in this snapshot.
    ///
    /// The stage commits each correspond to one of the possible stages in the
    /// index. If a file is not present in that stage, it's assumed that it's
    /// unchanged from the `HEAD` commit at the time which the snapshot was
    /// taken.
    ///
    /// The metadata is stored in the commit message.
    pub base_commit: Commit<'repo>,

    /// The commit that was checked out at the time of this snapshot. It's
    /// possible that *no* commit was checked out (called an "unborn HEAD").
    /// This could happen when the repository has been freshly initialized, but
    /// no commits have yet been made.
    pub head_commit: Option<Commit<'repo>>,

    /// The branch that was checked out at the time of this snapshot, if any.
    /// This includes the `refs/heads/` prefix.
    pub head_reference_name: Option<ReferenceName>,

    /// The unstaged changes in the working copy.
    pub commit_unstaged: Commit<'repo>,

    /// The index contents at stage 0 (normal staged changes).
    pub commit_stage0: Commit<'repo>,

    /// The index contents at stage 1. For a merge conflict, this corresponds to
    /// the contents of the file at the common ancestor of the merged commits.
    pub commit_stage1: Commit<'repo>,

    /// The index contents at stage 2 ("ours").
    pub commit_stage2: Commit<'repo>,

    /// The index contents at stage 3 ("theirs", i.e. the commit being merged
    /// in).
    pub commit_stage3: Commit<'repo>,
}

/// The type of changes in the working copy, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkingCopyChangesType {
    /// There are no changes to tracked files in the working copy.
    None,

    /// There are unstaged changes to tracked files in the working copy.
    Unstaged,

    /// There are staged changes to tracked files in the working copy. (There may also be unstaged
    /// changes.)
    Staged,

    /// The working copy has unresolved merge conflicts.
    Conflicts,
}

impl<'repo> WorkingCopySnapshot<'repo> {
    #[instrument]
    pub(super) fn create(
        repo: &'repo Repo,
        index: &Index,
        head_info: &ResolvedReferenceInfo,
        status_entries: &[StatusEntry],
    ) -> eyre::Result<Self> {
        let head_commit = match head_info.oid {
            Some(oid) => Some(repo.find_commit_or_fail(oid)?),
            None => None,
        };
        let head_commit_oid: MaybeZeroOid = match &head_commit {
            Some(head_commit) => MaybeZeroOid::NonZero(head_commit.get_oid()),
            None => MaybeZeroOid::Zero,
        };
        let head_reference_name: Option<ReferenceName> = head_info.reference_name.clone();

        let commit_unstaged_oid: NonZeroOid = {
            Self::create_commit_for_unstaged_changes(repo, head_commit.as_ref(), status_entries)?
        };

        let commit_stage0 = Self::create_commit_for_stage(
            repo,
            index,
            head_commit.as_ref(),
            status_entries,
            Stage::Stage0,
        )?;
        let commit_stage1 = Self::create_commit_for_stage(
            repo,
            index,
            head_commit.as_ref(),
            status_entries,
            Stage::Stage1,
        )?;
        let commit_stage2 = Self::create_commit_for_stage(
            repo,
            index,
            head_commit.as_ref(),
            status_entries,
            Stage::Stage2,
        )?;
        let commit_stage3 = Self::create_commit_for_stage(
            repo,
            index,
            head_commit.as_ref(),
            status_entries,
            Stage::Stage3,
        )?;

        let trailers = {
            let mut result = vec![(BRANCHLESS_HEAD_TRAILER, head_commit_oid.to_string())];
            if let Some(head_reference_name) = &head_reference_name {
                result.push((
                    BRANCHLESS_HEAD_REF_TRAILER,
                    head_reference_name.as_str().to_owned(),
                ));
            }
            result.extend([
                (BRANCHLESS_UNSTAGED_TRAILER, commit_unstaged_oid.to_string()),
                (Stage::Stage0.get_trailer(), commit_stage0.to_string()),
                (Stage::Stage1.get_trailer(), commit_stage1.to_string()),
                (Stage::Stage2.get_trailer(), commit_stage2.to_string()),
                (Stage::Stage3.get_trailer(), commit_stage3.to_string()),
            ]);
            result
        };
        let signature = Signature::automated()?;
        let message = format!(
            "\
branchless: automated working copy snapshot

{}
",
            trailers
                .into_iter()
                .map(|(name, value)| format!("{name}: {value}"))
                .collect_vec()
                .join("\n"),
        );

        // Use the current HEAD as the tree for parent commit, so that we can
        // look at any of the stage commits and compare them to their immediate
        // parent to find their logical contents.
        let tree = match &head_commit {
            Some(head_commit) => head_commit.get_tree()?,
            None => make_empty_tree(repo)?,
        };

        let commit_stage0 = repo.find_commit_or_fail(commit_stage0)?;
        let commit_stage1 = repo.find_commit_or_fail(commit_stage1)?;
        let commit_stage2 = repo.find_commit_or_fail(commit_stage2)?;
        let commit_stage3 = repo.find_commit_or_fail(commit_stage3)?;
        let parents = {
            // Add these commits as parents to ensure that they're kept live for
            // as long as the snapshot commit itself is live.
            let mut parents = vec![
                &commit_stage0,
                &commit_stage1,
                &commit_stage2,
                &commit_stage3,
            ];
            if let Some(head_commit) = &head_commit {
                // Make the head commit the first parent, since that's
                // conventionally the mainline parent.
                parents.insert(0, head_commit);
            }
            parents
        };
        let commit_oid =
            repo.create_commit(&signature, &signature, &message, &tree, parents, None)?;

        Ok(WorkingCopySnapshot {
            base_commit: repo.find_commit_or_fail(commit_oid)?,
            head_commit: head_commit.clone(),
            head_reference_name,
            commit_unstaged: repo.find_commit_or_fail(commit_unstaged_oid)?,
            commit_stage0,
            commit_stage1,
            commit_stage2,
            commit_stage3,
        })
    }

    /// Attempt to load the provided commit as if it were the base commit for a
    /// [`WorkingCopySnapshot`]. Returns `None` if it was not.
    #[instrument]
    pub fn try_from_base_commit<'a>(
        repo: &'repo Repo,
        base_commit: &'a Commit<'repo>,
    ) -> eyre::Result<Option<WorkingCopySnapshot<'repo>>> {
        let trailers = base_commit.get_trailers()?;
        let find_commit = |trailer: &str| -> eyre::Result<Option<Commit>> {
            for (k, v) in trailers.iter() {
                if k != trailer {
                    continue;
                }

                let oid = MaybeZeroOid::from_str(v);
                let oid = match oid {
                    Ok(MaybeZeroOid::NonZero(oid)) => oid,
                    Ok(MaybeZeroOid::Zero) => return Ok(None),
                    Err(_) => continue,
                };

                let result = repo.find_commit_or_fail(oid)?;
                return Ok(Some(result));
            }
            Ok(None)
        };

        let head_commit = find_commit(BRANCHLESS_HEAD_TRAILER)?;
        let commit_unstaged = match find_commit(BRANCHLESS_UNSTAGED_TRAILER)? {
            Some(commit) => commit,
            None => return Ok(None),
        };
        let head_reference_name = trailers.iter().find_map(|(k, v)| {
            if k == BRANCHLESS_HEAD_REF_TRAILER {
                Some(ReferenceName::from(v.as_str()))
            } else {
                None
            }
        });

        let commit_stage0 = match find_commit(Stage::Stage0.get_trailer())? {
            Some(commit) => commit,
            None => return Ok(None),
        };
        let commit_stage1 = match find_commit(Stage::Stage1.get_trailer())? {
            Some(commit) => commit,
            None => return Ok(None),
        };
        let commit_stage2 = match find_commit(Stage::Stage2.get_trailer())? {
            Some(commit) => commit,
            None => return Ok(None),
        };
        let commit_stage3 = match find_commit(Stage::Stage3.get_trailer())? {
            Some(commit) => commit,
            None => return Ok(None),
        };

        Ok(Some(WorkingCopySnapshot {
            base_commit: base_commit.to_owned(),
            head_commit,
            head_reference_name,
            commit_unstaged,
            commit_stage0,
            commit_stage1,
            commit_stage2,
            commit_stage3,
        }))
    }

    #[instrument]
    fn create_commit_for_unstaged_changes(
        repo: &Repo,
        head_commit: Option<&Commit>,
        status_entries: &[StatusEntry],
    ) -> eyre::Result<NonZeroOid> {
        let changed_paths: Vec<_> = status_entries
            .iter()
            .filter(|entry| {
                // The working copy status is reported with respect to the
                // staged changes, not to the `HEAD` commit. That means that if
                // the working copy status is reported as modified and the
                // staged status is reported as unmodified, there actually *was*
                // a change on disk that we need to detect.
                //
                // On the other hand, if both are reported as modified, it's
                // possible that there's *only* a staged change.
                //
                // Thus, we simply take all status entries that might refer to a
                // file which has changed since `HEAD`. Later, we'll recompute
                // the blobs for those files and hydrate the tree object. If it
                // wasn't actually changed, then no harm will be done and that
                // entry in the tree will also be unchanged.
                entry.working_copy_status.is_changed() || entry.index_status.is_changed()
            })
            .flat_map(|entry| {
                entry
                    .paths()
                    .into_iter()
                    .map(|path| (path, entry.working_copy_file_mode))
            })
            .collect();
        let num_changes = changed_paths.len();

        let head_tree = head_commit.map(|commit| commit.get_tree()).transpose()?;
        let hydrate_entries = {
            let mut result = HashMap::new();
            for (path, file_mode) in changed_paths {
                let entry = if file_mode == FileMode::Unreadable {
                    // If the file was deleted from the index, it's possible
                    // that it might still exist on disk. However, if the mode
                    // is `Unreadable`, that means that we should ignore its
                    // existence on disk because it's no longer being tracked by
                    // the index.
                    None
                } else {
                    repo.create_blob_from_path(&path)?
                        .map(|blob_oid| (blob_oid, file_mode))
                };
                result.insert(path, entry);
            }
            result
        };
        let tree_unstaged = {
            let tree_oid = hydrate_tree(repo, head_tree.as_ref(), hydrate_entries)?;
            repo.find_tree_or_fail(tree_oid)?
        };

        let signature = Signature::automated()?;
        let message = format!(
            "branchless: working copy snapshot data: {}",
            Pluralize {
                determiner: None,
                amount: num_changes,
                unit: ("unstaged change", "unstaged changes"),
            }
        );
        let commit = repo.create_commit(
            &signature,
            &signature,
            &message,
            &tree_unstaged,
            Vec::from_iter(head_commit),
            None,
        )?;
        Ok(commit)
    }

    #[instrument]
    fn create_commit_for_stage(
        repo: &Repo,
        index: &Index,
        head_commit: Option<&Commit>,
        status_entries: &[StatusEntry],
        stage: Stage,
    ) -> eyre::Result<NonZeroOid> {
        let mut updated_entries = HashMap::new();
        for StatusEntry {
            path, index_status, ..
        } in status_entries
        {
            let index_entry = index.get_entry_in_stage(path, stage);

            let entry = match index_entry {
                None => match (stage, index_status) {
                    // Stage 0 should have a copy of every file in the working
                    // tree, so the absence of that file now means that it was
                    // staged as deleted.
                    (Stage::Stage0, _) => None,

                    // If this file was in a state of conflict, then having
                    // failed to find it in the index means that it was deleted
                    // in this stage.
                    (Stage::Stage1 | Stage::Stage2 | Stage::Stage3, FileStatus::Unmerged) => None,

                    // If this file wasn't in a state of conflict, then we
                    // should use the HEAD entry for this stage.
                    (
                        Stage::Stage1 | Stage::Stage2 | Stage::Stage3,
                        FileStatus::Added
                        | FileStatus::Copied
                        | FileStatus::Deleted
                        | FileStatus::Ignored
                        | FileStatus::Modified
                        | FileStatus::Renamed
                        | FileStatus::Unmodified
                        | FileStatus::Untracked,
                    ) => continue,
                },

                Some(IndexEntry {
                    oid: MaybeZeroOid::Zero,
                    file_mode: _,
                }) => None,

                Some(IndexEntry {
                    oid: MaybeZeroOid::NonZero(oid),
                    file_mode,
                }) => Some((oid, file_mode)),
            };

            updated_entries.insert(path.clone(), entry);
        }

        let num_stage_changes = updated_entries.len();
        let head_tree = match head_commit {
            Some(head_commit) => Some(head_commit.get_tree()?),
            None => None,
        };
        let tree_oid = hydrate_tree(repo, head_tree.as_ref(), updated_entries)?;
        let tree = repo.find_tree_or_fail(tree_oid)?;

        let signature = Signature::automated()?;
        let message = format!(
            "branchless: working copy snapshot data: {}",
            Pluralize {
                determiner: None,
                amount: num_stage_changes,
                unit: (
                    &format!("change in stage {}", i32::from(stage)),
                    &format!("changes in stage {}", i32::from(stage)),
                ),
            }
        );
        let commit_oid = repo.create_commit(
            &signature,
            &signature,
            &message,
            &tree,
            match head_commit {
                Some(parent_commit) => vec![parent_commit],
                None => vec![],
            },
            None,
        )?;
        Ok(commit_oid)
    }

    /// Determine what kind of changes to the working copy the user made in this snapshot.
    #[instrument]
    pub fn get_working_copy_changes_type(&self) -> eyre::Result<WorkingCopyChangesType> {
        let base_tree_oid = self.base_commit.get_tree_oid();
        let unstaged_tree_oid = self.commit_unstaged.get_tree_oid();
        let stage0_tree_oid = self.commit_stage0.get_tree_oid();
        let stage1_tree_oid = self.commit_stage1.get_tree_oid();
        let stage2_tree_oid = self.commit_stage2.get_tree_oid();
        let stage3_tree_oid = self.commit_stage3.get_tree_oid();

        if base_tree_oid != stage1_tree_oid
            || base_tree_oid != stage2_tree_oid
            || base_tree_oid != stage3_tree_oid
        {
            Ok(WorkingCopyChangesType::Conflicts)
        } else if base_tree_oid != stage0_tree_oid {
            Ok(WorkingCopyChangesType::Staged)
        } else if base_tree_oid != unstaged_tree_oid {
            Ok(WorkingCopyChangesType::Unstaged)
        } else {
            Ok(WorkingCopyChangesType::None)
        }
    }
}
