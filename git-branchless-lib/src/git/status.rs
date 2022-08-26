use std::path::PathBuf;
use std::str::FromStr;

use lazy_static::lazy_static;
use os_str_bytes::OsStrBytes;
use regex::bytes::Regex;
use tracing::{instrument, warn};

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

/// Wrapper around [git2::FileMode].
#[allow(missing_docs)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
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

impl From<FileMode> for u32 {
    fn from(file_mode: FileMode) -> Self {
        i32::from(file_mode).try_into().unwrap()
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

impl ToString for FileMode {
    fn to_string(&self) -> String {
        match self {
            FileMode::Unreadable => "000000".to_string(),
            FileMode::Tree => "040000".to_string(),
            FileMode::Blob => "100644".to_string(),
            FileMode::BlobExecutable => "100755".to_string(),
            FileMode::Link => "120000".to_string(),
            FileMode::Commit => "160000".to_string(),
        }
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
                r#"^(?P<prefix>1|2|u) "#,                                    // Prefix.
                r#"(?P<index_status>[\w.])(?P<working_copy_status>[\w.]) "#, // Status indicators.
                r#"[\w.]+ "#,                                                // Submodule state.
                r#"(\d{6} ){2,3}(?P<working_copy_filemode>\d{6}) "#,         // HEAD, Index, and Working Copy file modes;
                                                                             // or stage1, stage2, stage3, and working copy file modes.
                r#"([\w\d]+ ){2,3}"#,                                        // HEAD and Index object IDs, and optionally the rename/copy score.
                r#"(?P<path>[^\x00]+)(\x00(?P<orig_path>[^\x00]+))?$"#       // Path and original path (for renames/copies).
            ))
            .expect("porcelain v2 status line regex");
        }

        let status_line_parts = STATUS_PORCELAIN_V2_REGEXP
            .captures(line)
            .ok_or_else(|| eyre::eyre!("unable to parse status line into parts"))?;

        let index_status: FileStatus = match status_line_parts.name("prefix") {
            Some(m) if m.as_bytes() == b"u" => FileStatus::Unmerged,
            _ => status_line_parts
                .name("index_status")
                .and_then(|m| m.as_bytes().iter().next().copied())
                .ok_or_else(|| eyre::eyre!("no index status indicator"))?
                .into(),
        };
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

#[cfg(test)]
mod tests {
    use crate::core::effects::Effects;
    use crate::core::formatting::Glyphs;
    use crate::git::WorkingCopyChangesType;
    use crate::testing::make_git;

    use super::*;

    #[test]
    fn test_parse_status_line() {
        assert_eq!(
            StatusEntry::try_from(
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
            StatusEntry::try_from(
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

        let entry: StatusEntry = StatusEntry::try_from(
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

        assert_eq!(
            StatusEntry::try_from(
                "u A. N... 100755 100755 100755 100755 51fcbe2362663a19d132767b69c2c7829023f3da 51fcbe2362663a19d132767b69c2c7829023f3da 9daeafb9864cf43055ae93beb0afd6c7d144bfa4 repo.rs".as_bytes(),
            ).unwrap(),
            StatusEntry {
                index_status: FileStatus::Unmerged,
                working_copy_status: FileStatus::Unmodified,
                path: "repo.rs".into(),
                orig_path: None,
                working_copy_file_mode: FileMode::BlobExecutable,
            }
        );
    }

    #[test]
    fn test_get_status() -> eyre::Result<()> {
        let git = make_git()?;
        let git_run_info = git.get_git_run_info();
        git.init_repo()?;
        git.commit_file("test1", 1)?;

        let glyphs = Glyphs::text();
        let effects = Effects::new_suppress_for_test(glyphs);
        let repo = git.get_repo()?;

        let (snapshot, status) = repo.get_status(
            &effects,
            &git_run_info,
            &repo.get_index()?,
            &repo.get_head_info()?,
            None,
        )?;
        assert_eq!(
            snapshot.get_working_copy_changes_type()?,
            WorkingCopyChangesType::None
        );
        assert_eq!(status, vec![]);
        insta::assert_debug_snapshot!(snapshot, @r###"
        WorkingCopySnapshot {
            base_commit: Commit {
                inner: Commit {
                    id: ad8334119626cc9aee5322f9ed35273de834ea36,
                    summary: "branchless: automated working copy snapshot",
                },
            },
            head_commit: Some(
                Commit {
                    inner: Commit {
                        id: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                        summary: "create test1.txt",
                    },
                },
            ),
            head_reference_name: Some(
                ReferenceName(
                    "refs/heads/master",
                ),
            ),
            commit_unstaged: Commit {
                inner: Commit {
                    id: cd8605eef8b78e22427fa3846f1a23f95e88aa7e,
                    summary: "branchless: working copy snapshot data: 0 unstaged changes",
                },
            },
            commit_stage0: Commit {
                inner: Commit {
                    id: a4edb48b44f5b19d0c2c25fd65251d0bfaba68c1,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 0",
                },
            },
            commit_stage1: Commit {
                inner: Commit {
                    id: e1e0c856237e53e2c889a723ee7ec50a21c1f952,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 1",
                },
            },
            commit_stage2: Commit {
                inner: Commit {
                    id: e5dda473c3266aafa14b827b1a009e35a3c61679,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 2",
                },
            },
            commit_stage3: Commit {
                inner: Commit {
                    id: 19b98ca24cc7b241122593fc1a9307e24e26a846,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 3",
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
            &effects,
            &git_run_info,
            &repo.get_index()?,
            &repo.get_head_info()?,
            None,
        )?;
        assert_eq!(
            snapshot.get_working_copy_changes_type()?,
            WorkingCopyChangesType::Staged
        );
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
                    id: e378f780ff4d810e12d36e89334d8971d7add0d1,
                    summary: "branchless: automated working copy snapshot",
                },
            },
            head_commit: Some(
                Commit {
                    inner: Commit {
                        id: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                        summary: "create test1.txt",
                    },
                },
            ),
            head_reference_name: Some(
                ReferenceName(
                    "refs/heads/master",
                ),
            ),
            commit_unstaged: Commit {
                inner: Commit {
                    id: 8f4827eba90e1ce5bbd0d76fc29d587c8ad9135e,
                    summary: "branchless: working copy snapshot data: 4 unstaged changes",
                },
            },
            commit_stage0: Commit {
                inner: Commit {
                    id: ccfd588cf59116f67664ac718c404b09fc9e35d2,
                    summary: "branchless: working copy snapshot data: 3 changes in stage 0",
                },
            },
            commit_stage1: Commit {
                inner: Commit {
                    id: e1e0c856237e53e2c889a723ee7ec50a21c1f952,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 1",
                },
            },
            commit_stage2: Commit {
                inner: Commit {
                    id: e5dda473c3266aafa14b827b1a009e35a3c61679,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 2",
                },
            },
            commit_stage3: Commit {
                inner: Commit {
                    id: 19b98ca24cc7b241122593fc1a9307e24e26a846,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 3",
                },
            },
        }
        "###);

        Ok(())
    }
}
