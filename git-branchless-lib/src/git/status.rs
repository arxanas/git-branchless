use std::path::{Path, PathBuf};
use std::str::FromStr;

use lazy_static::lazy_static;
use os_str_bytes::OsStrBytes;
use regex::bytes::Regex;
use tracing::{instrument, warn};

use super::MaybeZeroOid;

/// A Git file status indicator.
/// See <https://git-scm.com/docs/git-status#_short_format>.
#[allow(missing_docs)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FileStatus {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
    Ignored,
}

impl FileStatus {
    /// Determine if this status corresponds to a "changed" status, which means
    /// that it should be included in a commit.
    pub fn is_changed(&self) -> bool {
        match self {
            FileStatus::Added
            | FileStatus::Copied
            | FileStatus::Deleted
            | FileStatus::Modified
            | FileStatus::Renamed => true,
            FileStatus::Ignored
            | FileStatus::Unmerged
            | FileStatus::Unmodified
            | FileStatus::Untracked => false,
        }
    }
}

impl From<u8> for FileStatus {
    fn from(status: u8) -> Self {
        match status {
            b'.' => FileStatus::Unmodified,
            b'M' => FileStatus::Modified,
            b'A' => FileStatus::Added,
            b'D' => FileStatus::Deleted,
            b'R' => FileStatus::Renamed,
            b'C' => FileStatus::Copied,
            b'U' => FileStatus::Unmerged,
            b'?' => FileStatus::Untracked,
            b'!' => FileStatus::Ignored,
            _ => {
                warn!(?status, "invalid status indicator");
                FileStatus::Untracked
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FileMode {
    Unreadable,
    Tree,
    Blob,
    BlobExecutable,
    Link,
    Commit,
}

impl From<git2::FileMode> for FileMode {
    fn from(file_mode: git2::FileMode) -> Self {
        match file_mode {
            git2::FileMode::Blob => FileMode::Blob,
            git2::FileMode::BlobExecutable => FileMode::BlobExecutable,
            git2::FileMode::Commit => FileMode::Commit,
            git2::FileMode::Link => FileMode::Link,
            git2::FileMode::Tree => FileMode::Tree,
            git2::FileMode::Unreadable => FileMode::Unreadable,
        }
    }
}

impl From<i32> for FileMode {
    fn from(file_mode: i32) -> Self {
        if file_mode == i32::from(git2::FileMode::Blob) {
            FileMode::Blob
        } else if file_mode == i32::from(git2::FileMode::BlobExecutable) {
            FileMode::BlobExecutable
        } else if file_mode == i32::from(git2::FileMode::Commit) {
            FileMode::Commit
        } else if file_mode == i32::from(git2::FileMode::Link) {
            FileMode::Link
        } else if file_mode == i32::from(git2::FileMode::Tree) {
            FileMode::Tree
        } else {
            FileMode::Unreadable
        }
    }
}

impl From<FileMode> for i32 {
    fn from(file_mode: FileMode) -> Self {
        match file_mode {
            FileMode::Blob => git2::FileMode::Blob.into(),
            FileMode::BlobExecutable => git2::FileMode::BlobExecutable.into(),
            FileMode::Commit => git2::FileMode::Commit.into(),
            FileMode::Link => git2::FileMode::Link.into(),
            FileMode::Tree => git2::FileMode::Tree.into(),
            FileMode::Unreadable => git2::FileMode::Unreadable.into(),
        }
    }
}

impl FromStr for FileMode {
    type Err = eyre::Error;

    // Parses the string representation of a filemode for a status entry.
    // Git only supports a small subset of Unix octal file mode permissions.
    // See http://git-scm.com/book/en/v2/Git-Internals-Git-Objects
    fn from_str(file_mode: &str) -> eyre::Result<Self> {
        let file_mode = match file_mode {
            "000000" => FileMode::Unreadable,
            "040000" => FileMode::Tree,
            "100644" => FileMode::Blob,
            "100755" => FileMode::BlobExecutable,
            "120000" => FileMode::Link,
            "160000" => FileMode::Commit,
            _ => eyre::bail!("unknown file mode: {}", file_mode),
        };
        Ok(file_mode)
    }
}

/// The status of a file in the repo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    /// The status of the file in the index.
    pub index_status: FileStatus,
    /// The status of the file in the working copy.
    pub working_copy_status: FileStatus,
    /// The file mode of the file in the working copy.
    pub working_copy_file_mode: FileMode,
    /// The file path.
    pub path: PathBuf,
    /// The original path of the file (for renamed files).
    pub orig_path: Option<PathBuf>,
}

impl StatusEntry {
    /// Returns the paths associated with the status entry.
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut result = vec![self.path.clone()];
        if let Some(orig_path) = &self.orig_path {
            result.push(orig_path.clone());
        }
        result
    }
}

impl TryFrom<&[u8]> for StatusEntry {
    type Error = eyre::Error;

    #[instrument]
    fn try_from(line: &[u8]) -> eyre::Result<StatusEntry> {
        lazy_static! {
            /// Parses an entry of the git porcelain v2 status format.
            /// See https://git-scm.com/docs/git-status#_porcelain_format_version_2
            static ref STATUS_PORCELAIN_V2_REGEXP: Regex = Regex::new(concat!(
                r#"^(1|2) (?P<index_status>[\w.])(?P<working_copy_status>[\w.]) "#, // Prefix and status indicators.
                r#"[\w.]+ "#,                                                       // Submodule state.
                r#"(\d{6} ){2}(?P<working_copy_filemode>\d{6}) "#,                  // HEAD, Index, and Working Copy file modes.
                r#"([\w\d]+ ){2,3}"#,                                               // HEAD and Index object IDs, and optionally the rename/copy score.
                r#"(?P<path>[^\x00]+)(\x00(?P<orig_path>[^\x00]+))?$"#              // Path and original path (for renames/copies).
            ))
            .expect("porcelain v2 status line regex");
        }

        let status_line_parts = STATUS_PORCELAIN_V2_REGEXP
            .captures(line)
            .ok_or_else(|| eyre::eyre!("unable to parse status line into parts"))?;

        let index_status: FileStatus = status_line_parts
            .name("index_status")
            .and_then(|m| m.as_bytes().iter().next().copied())
            .ok_or_else(|| eyre::eyre!("no index status indicator"))?
            .into();
        let working_copy_status: FileStatus = status_line_parts
            .name("working_copy_status")
            .and_then(|m| m.as_bytes().iter().next().copied())
            .ok_or_else(|| eyre::eyre!("no working copy status indicator"))?
            .into();
        let working_copy_file_mode = status_line_parts
            .name("working_copy_filemode")
            .ok_or_else(|| eyre::eyre!("no working copy filemode in status line"))
            .and_then(|m| {
                std::str::from_utf8(m.as_bytes())
                    .map_err(|err| {
                        eyre::eyre!("unable to decode working copy file mode: {:?}", err)
                    })
                    .and_then(|working_copy_file_mode| working_copy_file_mode.parse::<FileMode>())
            })?;
        let path = status_line_parts
            .name("path")
            .ok_or_else(|| eyre::eyre!("no path in status line"))?
            .as_bytes();
        let orig_path = status_line_parts
            .name("orig_path")
            .map(|orig_path| orig_path.as_bytes());

        Ok(StatusEntry {
            index_status,
            working_copy_status,
            working_copy_file_mode,
            path: PathBuf::from(OsStrBytes::from_raw_bytes(path)?),
            orig_path: orig_path.map(|orig_path| {
                OsStrBytes::from_raw_bytes(orig_path)
                    .map(PathBuf::from)
                    .expect("unable to convert orig_path to PathBuf")
            }),
        })
    }
}

#[derive(Copy, Clone, Debug)]
pub enum Stage {
    Stage0,
    Stage1,
    Stage2,
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

pub struct IndexEntry {
    pub(super) oid: MaybeZeroOid,
    pub(super) file_mode: FileMode,
}

pub struct Index {
    pub(super) inner: git2::Index,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<Index>")
    }
}

impl Index {
    pub fn has_conflicts(&self) -> bool {
        self.inner.has_conflicts()
    }

    pub fn get_entry(&self, path: &Path) -> Option<IndexEntry> {
        self.get_entry_in_stage(path, Stage::Stage0)
    }

    pub fn get_entry_in_stage(&self, path: &Path, stage: Stage) -> Option<IndexEntry> {
        self.inner
            .get_path(path, i32::from(stage))
            .map(|entry| IndexEntry {
                oid: entry.id.into(),
                file_mode: {
                    // `libgit2` uses u32 for file modes in index entries, but
                    // i32 for file modes in tree entries for some reason.
                    let mode = i32::try_from(entry.mode).unwrap();
                    FileMode::try_from(mode).unwrap()
                },
            })
    }
}

#[cfg(test)]
mod tests {
    use crate::git::GitRunInfo;
    use crate::testing::make_git;

    use super::*;

    #[test]
    fn test_parse_status_line() {
        assert_eq!(
            TryInto::<StatusEntry>::try_into(
                "1 .M N... 100644 100644 100644 51fcbe2362663a19d132767b69c2c7829023f3da 51fcbe2362663a19d132767b69c2c7829023f3da repo.rs".as_bytes(),
            ).unwrap(),
            StatusEntry {
                index_status: FileStatus::Unmodified,
                working_copy_status: FileStatus::Modified,
                path: "repo.rs".into(),
                orig_path: None,
                working_copy_file_mode: FileMode::Blob,
            }
        );

        assert_eq!(
            TryInto::<StatusEntry>::try_into(
                "1 A. N... 100755 100755 100755 51fcbe2362663a19d132767b69c2c7829023f3da 51fcbe2362663a19d132767b69c2c7829023f3da repo.rs".as_bytes(),
            ).unwrap(),
            StatusEntry {
                index_status: FileStatus::Added,
                working_copy_status: FileStatus::Unmodified,
                path: "repo.rs".into(),
                orig_path: None,
                working_copy_file_mode: FileMode::BlobExecutable,
            }
        );

        let entry: StatusEntry = TryInto::<StatusEntry>::try_into(
            "2 RD N... 100644 100644 100644 9daeafb9864cf43055ae93beb0afd6c7d144bfa4 9daeafb9864cf43055ae93beb0afd6c7d144bfa4 R100 new_file.rs\x00old_file.rs".as_bytes(),
        ).unwrap();
        assert_eq!(
            entry,
            StatusEntry {
                index_status: FileStatus::Renamed,
                working_copy_status: FileStatus::Deleted,
                path: "new_file.rs".into(),
                orig_path: Some("old_file.rs".into()),
                working_copy_file_mode: FileMode::Blob,
            }
        );
        assert_eq!(
            entry.paths(),
            vec![PathBuf::from("new_file.rs"), PathBuf::from("old_file.rs")]
        );
    }

    #[test]
    fn test_get_status() -> eyre::Result<()> {
        let git = make_git()?;
        let git_run_info = GitRunInfo {
            path_to_git: git.path_to_git.clone(),
            working_directory: git.repo_path.clone(),
            env: git.get_base_env(0).into_iter().collect(),
        };
        git.init_repo()?;
        git.commit_file("test1", 1)?;

        let repo = git.get_repo()?;

        let (snapshot, status) = repo.get_status(
            &git_run_info,
            &repo.get_index()?,
            &repo.get_head_info()?,
            None,
        )?;
        assert_eq!(status, vec![]);
        insta::assert_debug_snapshot!(snapshot, @r###"
        WorkingCopySnapshot {
            base_commit: Commit {
                inner: Commit {
                    id: 98d860e0cc6865538d5571e192705f8a090d95d8,
                    summary: "branchless: automated working copy commit",
                },
            },
            commit_stage0: Commit {
                inner: Commit {
                    id: 428260eed0b9a234827fbc529428fb9b44917e7e,
                    summary: "branchless: automated working copy commit (0 changes)",
                },
            },
            commit_stage1: Commit {
                inner: Commit {
                    id: 428260eed0b9a234827fbc529428fb9b44917e7e,
                    summary: "branchless: automated working copy commit (0 changes)",
                },
            },
            commit_stage2: Commit {
                inner: Commit {
                    id: 428260eed0b9a234827fbc529428fb9b44917e7e,
                    summary: "branchless: automated working copy commit (0 changes)",
                },
            },
            commit_stage3: Commit {
                inner: Commit {
                    id: 428260eed0b9a234827fbc529428fb9b44917e7e,
                    summary: "branchless: automated working copy commit (0 changes)",
                },
            },
        }
        "###);

        git.write_file("new_file", "another file")?;
        git.run(&["add", "new_file.txt"])?;
        git.write_file("untracked", "should not show up in status")?;
        git.delete_file("initial")?;
        git.run(&["mv", "test1.txt", "renamed.txt"])?;

        let (snapshot, status) = repo.get_status(
            &git_run_info,
            &repo.get_index()?,
            &repo.get_head_info()?,
            None,
        )?;
        assert_eq!(
            status,
            vec![
                StatusEntry {
                    index_status: FileStatus::Unmodified,
                    working_copy_status: FileStatus::Deleted,
                    working_copy_file_mode: FileMode::Unreadable,
                    path: "initial.txt".into(),
                    orig_path: None
                },
                StatusEntry {
                    index_status: FileStatus::Added,
                    working_copy_status: FileStatus::Unmodified,
                    working_copy_file_mode: FileMode::Blob,
                    path: "new_file.txt".into(),
                    orig_path: None
                },
                StatusEntry {
                    index_status: FileStatus::Renamed,
                    working_copy_status: FileStatus::Unmodified,
                    working_copy_file_mode: FileMode::Blob,
                    path: "renamed.txt".into(),
                    orig_path: Some("test1.txt".into())
                }
            ]
        );
        insta::assert_debug_snapshot!(snapshot, @r###"
        WorkingCopySnapshot {
            base_commit: Commit {
                inner: Commit {
                    id: ff138a604cadfcd219d9a63b09b439af0fb0bafa,
                    summary: "branchless: automated working copy commit",
                },
            },
            commit_stage0: Commit {
                inner: Commit {
                    id: 1b6c0923419bab0907c2b20a076817845c39dae8,
                    summary: "branchless: automated working copy commit (1 change)",
                },
            },
            commit_stage1: Commit {
                inner: Commit {
                    id: 944ef4f6dff4ae9f07f0e3dac3ef7e7d9333ba94,
                    summary: "branchless: automated working copy commit (0 changes)",
                },
            },
            commit_stage2: Commit {
                inner: Commit {
                    id: 944ef4f6dff4ae9f07f0e3dac3ef7e7d9333ba94,
                    summary: "branchless: automated working copy commit (0 changes)",
                },
            },
            commit_stage3: Commit {
                inner: Commit {
                    id: 944ef4f6dff4ae9f07f0e3dac3ef7e7d9333ba94,
                    summary: "branchless: automated working copy commit (0 changes)",
                },
            },
        }
        "###);

        Ok(())
    }
}
