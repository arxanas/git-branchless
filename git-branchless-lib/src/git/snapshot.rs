//! Implementation of working copy snapshots. The ideas are based off of those
//! in Jujutsu: https://github.com/martinvonz/jj/blob/main/docs/working-copy.md
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
//!  jump back into merge conflict resolution which was happening at some past
//!  time.
//!  2. To unify the implementations of operations across commits and the
//!  working copy. For example, a `git split` command which splits one commit
//!  into multiple could also be used to split the working copy into multiple
//!  commits.

use std::collections::HashMap;
use std::str::FromStr;

use tracing::instrument;

use crate::core::formatting::Pluralize;

use super::repo::Signature;
use super::status::{Index, IndexEntry, Stage};
use super::tree::{hydrate_tree, make_empty_tree};
use super::{Commit, MaybeZeroOid, NonZeroOid, Repo, ResolvedReferenceInfo, StatusEntry};

/// A special `Commit` which represents the status of the working copy at a
/// given point in time. This means that it can include changes in any stage.
#[derive(Clone, Debug)]
pub struct WorkingCopySnapshot<'repo> {
    /// The commit which contains the metadata about the `HEAD` commit and all
    /// the "stage commits" included in this snapshot.
    ///
    /// This commit itself contains identical content to the `HEAD` commit (if
    /// any). The `HEAD` commit (if any) is this commit's first parent.
    ///
    /// The stage commits each correspond to one of the possible stages in the
    /// index. If a file is not present in that stage, it's assumed that it's
    /// unchanged from the `HEAD` commit at the time which the snapshot was
    /// taken.
    ///
    /// The metadata is stored in the commit message.
    pub base_commit: Commit<'repo>,

    /// The index contents at stage 0 (unstaged).
    pub commit_stage0: Commit<'repo>,

    /// The index contents at stage 1 (staged).
    pub commit_stage1: Commit<'repo>,

    /// The index contents at stage 2 ("ours").
    pub commit_stage2: Commit<'repo>,

    /// The index contents at stage 3 ("theirs", i.e. the commit being merged in).
    pub commit_stage3: Commit<'repo>,
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

        let signature = Signature::automated()?;
        let message = format!(
            "\
branchless: automated working copy commit

{}: {}
{}: {}
{}: {}
{}: {}
",
            Stage::Stage0.get_trailer(),
            commit_stage0,
            Stage::Stage1.get_trailer(),
            commit_stage1,
            Stage::Stage2.get_trailer(),
            commit_stage2,
            Stage::Stage3.get_trailer(),
            commit_stage3,
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
            repo.create_commit(None, &signature, &signature, &message, &tree, parents)?;

        Ok(WorkingCopySnapshot {
            base_commit: repo.find_commit_or_fail(commit_oid)?,
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
        let find_commit = |stage: Stage| -> eyre::Result<Option<Commit>> {
            for (k, v) in trailers.iter() {
                if k != stage.get_trailer() {
                    continue;
                }

                let oid = NonZeroOid::from_str(v);
                let oid = match oid {
                    Ok(oid) => oid,
                    Err(_) => continue,
                };

                let result = repo.find_commit_or_fail(oid)?;
                return Ok(Some(result));
            }
            Ok(None)
        };

        let commit_stage0 = match find_commit(Stage::Stage0)? {
            Some(commit) => commit,
            None => return Ok(None),
        };
        let commit_stage1 = match find_commit(Stage::Stage1)? {
            Some(commit) => commit,
            None => return Ok(None),
        };
        let commit_stage2 = match find_commit(Stage::Stage2)? {
            Some(commit) => commit,
            None => return Ok(None),
        };
        let commit_stage3 = match find_commit(Stage::Stage3)? {
            Some(commit) => commit,
            None => return Ok(None),
        };

        Ok(Some(WorkingCopySnapshot {
            base_commit: base_commit.to_owned(),
            commit_stage0,
            commit_stage1,
            commit_stage2,
            commit_stage3,
        }))
    }

    #[instrument]
    fn create_commit_for_stage(
        repo: &Repo,
        index: &Index,
        parent_commit: Option<&Commit>,
        status_entries: &[StatusEntry],
        stage: Stage,
    ) -> eyre::Result<NonZeroOid> {
        let mut updated_entries = HashMap::new();
        let mut num_stage_changes = 0;
        for StatusEntry { path, .. } in status_entries {
            let index_entry = index.get_entry_in_stage(path, stage);
            if index_entry.is_some() {
                num_stage_changes += 1;
            }

            let entry = match index_entry {
                Some(IndexEntry {
                    oid: MaybeZeroOid::Zero,
                    file_mode: _,
                })
                | None => None,
                Some(IndexEntry {
                    oid: MaybeZeroOid::NonZero(oid),
                    file_mode,
                }) => Some((oid, file_mode)),
            };
            updated_entries.insert(path.clone(), entry);
        }

        let parent_tree = match parent_commit {
            Some(parent_commit) => Some(parent_commit.get_tree()?),
            None => None,
        };
        let tree_oid = hydrate_tree(repo, parent_tree.as_ref(), updated_entries)?;
        let tree = repo.find_tree_or_fail(tree_oid)?;
        let signature = Signature::automated()?;
        let message = format!(
            "branchless: automated working copy commit ({})",
            Pluralize {
                determiner: None,
                amount: num_stage_changes,
                unit: ("change", "changes"),
            },
        );
        let commit_oid = repo.create_commit(
            None,
            &signature,
            &signature,
            &message,
            &tree,
            match parent_commit {
                Some(parent_commit) => vec![parent_commit],
                None => vec![],
            },
        )?;
        Ok(commit_oid)
    }
}
