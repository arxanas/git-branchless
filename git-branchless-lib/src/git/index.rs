use std::path::{Path, PathBuf};

use eyre::Context;
use tracing::instrument;

use crate::core::eventlog::EventTransactionId;

use super::{FileMode, GitRunInfo, GitRunOpts, GitRunResult, MaybeZeroOid, NonZeroOid, Repo, Tree};

/// The possible stages for items in the index.
#[derive(Copy, Clone, Debug)]
pub enum Stage {
    /// Normal staged change.
    Stage0,

    /// For a merge conflict, the contents of the file at the common ancestor of the merged commits.
    Stage1,

    /// "Our" changes.
    Stage2,

    /// "Their" changes (from the commit being merged in).
    Stage3,
}

impl Stage {
    pub(super) fn get_trailer(&self) -> &'static str {
        match self {
            Stage::Stage0 => "Branchless-stage-0",
            Stage::Stage1 => "Branchless-stage-1",
            Stage::Stage2 => "Branchless-stage-2",
            Stage::Stage3 => "Branchless-stage-3",
        }
    }
}

impl From<Stage> for i32 {
    fn from(stage: Stage) -> Self {
        match stage {
            Stage::Stage0 => 0,
            Stage::Stage1 => 1,
            Stage::Stage2 => 2,
            Stage::Stage3 => 3,
        }
    }
}

/// An entry in the Git index.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IndexEntry {
    pub(super) oid: MaybeZeroOid,
    pub(super) file_mode: FileMode,
}

/// The Git index.
pub struct Index {
    pub(super) inner: git2::Index,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<Index>")
    }
}

impl Index {
    /// Whether or not there are unresolved merge conflicts in the index.
    pub fn has_conflicts(&self) -> bool {
        self.inner.has_conflicts()
    }

    /// Get the (stage 0) entry for the given path.
    pub fn get_entry(&self, path: &Path) -> Option<IndexEntry> {
        self.get_entry_in_stage(path, Stage::Stage0)
    }

    /// Get the entry for the given path in the given stage.
    pub fn get_entry_in_stage(&self, path: &Path, stage: Stage) -> Option<IndexEntry> {
        self.inner
            .get_path(path, i32::from(stage))
            .map(|entry| IndexEntry {
                oid: entry.id.into(),
                file_mode: {
                    // `libgit2` uses u32 for file modes in index entries, but
                    // i32 for file modes in tree entries for some reason.
                    let mode = i32::try_from(entry.mode).unwrap();
                    FileMode::from(mode)
                },
            })
    }

    /// Update the index from the given tree and write it to disk.
    pub fn update_from_tree(&mut self, tree: &Tree) -> eyre::Result<()> {
        self.inner.read_tree(&tree.inner)?;
        self.inner.write().wrap_err("writing index")
    }
}

/// The command to update the index, as defined by `git update-index`.
#[allow(missing_docs)]
#[derive(Clone, Debug)]
pub enum UpdateIndexCommand {
    Delete {
        path: PathBuf,
    },
    Update {
        path: PathBuf,
        stage: Stage,
        mode: FileMode,
        oid: NonZeroOid,
    },
}

/// Update the index. This handles updates to stages other than 0.
///
/// libgit2 doesn't offer a good way of updating the index for higher stages, so
/// internally we use `git update-index` directly.
#[instrument]
pub fn update_index(
    git_run_info: &GitRunInfo,
    repo: &Repo,
    index: &Index,
    event_tx_id: EventTransactionId,
    commands: &[UpdateIndexCommand],
) -> eyre::Result<()> {
    let stdin = {
        let mut buf = Vec::new();
        for command in commands {
            use std::io::Write;

            match command {
                UpdateIndexCommand::Delete { path } => {
                    write!(
                        &mut buf,
                        "0 {zero} 0\t{path}\0",
                        zero = MaybeZeroOid::Zero,
                        path = path.display(),
                    )?;
                }

                UpdateIndexCommand::Update {
                    path,
                    stage,
                    mode,
                    oid,
                } => {
                    write!(
                        &mut buf,
                        "{mode} {sha1} {stage}\t{path}\0",
                        sha1 = oid,
                        stage = i32::from(*stage),
                        path = path.display(),
                    )?;
                }
            }
        }
        buf
    };

    let GitRunResult { .. } = git_run_info
        .run_silent(
            repo,
            Some(event_tx_id),
            &["update-index", "-z", "--index-info"],
            GitRunOpts {
                treat_git_failure_as_error: true,
                stdin: Some(stdin),
            },
        )
        .wrap_err("Updating index")?;
    Ok(())
}
