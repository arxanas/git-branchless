use std::collections::HashMap;

use crate::core::formatting::Pluralize;

use super::repo::Signature;
use super::status::{Index, IndexEntry, Stage};
use super::tree::{hydrate_tree, make_empty_tree};
use super::{Commit, MaybeZeroOid, NonZeroOid, Repo, ResolvedReferenceInfo, StatusEntry};

/// A special `Commit` which represents the status of the working copy at a
/// given point in time. This means that it can include changes in any stage.
#[derive(Clone, Debug)]
pub struct WorkingCopySnapshot<'repo> {
    pub base_commit: Commit<'repo>,
    pub commit_stage0: Commit<'repo>,
    pub commit_stage1: Commit<'repo>,
    pub commit_stage2: Commit<'repo>,
    pub commit_stage3: Commit<'repo>,
}

impl<'repo> WorkingCopySnapshot<'repo> {
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
