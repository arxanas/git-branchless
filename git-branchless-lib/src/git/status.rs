use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;

use bstr::ByteVec;
use lazy_static::lazy_static;
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
    BlobGroupWritable,
    Link,
    Commit,
}

impl From<git2::FileMode> for FileMode {
    fn from(file_mode: git2::FileMode) -> Self {
        match file_mode {
            git2::FileMode::Blob => FileMode::Blob,
            git2::FileMode::BlobExecutable => FileMode::BlobExecutable,
            git2::FileMode::BlobGroupWritable => FileMode::BlobGroupWritable,
            git2::FileMode::Commit => FileMode::Commit,
            git2::FileMode::Link => FileMode::Link,
            git2::FileMode::Tree => FileMode::Tree,
            git2::FileMode::Unreadable => FileMode::Unreadable,
        }
    }
}

impl From<FileMode> for git2::FileMode {
    fn from(file_mode: FileMode) -> Self {
        match file_mode {
            FileMode::Blob => git2::FileMode::Blob,
            FileMode::BlobExecutable => git2::FileMode::BlobExecutable,
            FileMode::BlobGroupWritable => git2::FileMode::BlobGroupWritable,
            FileMode::Commit => git2::FileMode::Commit,
            FileMode::Link => git2::FileMode::Link,
            FileMode::Tree => git2::FileMode::Tree,
            FileMode::Unreadable => git2::FileMode::Unreadable,
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
            FileMode::BlobGroupWritable => git2::FileMode::BlobGroupWritable.into(),
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

impl From<scm_record::FileMode> for FileMode {
    fn from(file_mode: scm_record::FileMode) -> Self {
        match file_mode {
            scm_record::FileMode::Unix(file_mode) => {
                let file_mode: i32 = file_mode.try_into().unwrap();
                Self::from(file_mode)
            }
            scm_record::FileMode::Absent => FileMode::Unreadable,
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
            "100664" => FileMode::BlobGroupWritable,
            "120000" => FileMode::Link,
            "160000" => FileMode::Commit,
            _ => eyre::bail!("unknown file mode: {}", file_mode),
        };
        Ok(file_mode)
    }
}

impl Display for FileMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileMode::Unreadable => write!(f, "000000"),
            FileMode::Tree => write!(f, "040000"),
            FileMode::Blob => write!(f, "100644"),
            FileMode::BlobExecutable => write!(f, "100755"),
            FileMode::BlobGroupWritable => write!(f, "100664"),
            FileMode::Link => write!(f, "120000"),
            FileMode::Commit => write!(f, "160000"),
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
    /// Create a status entry for a currently-untracked, to-be-added file.
    pub fn new_untracked(filename: String) -> Self {
        StatusEntry {
            index_status: FileStatus::Untracked,
            working_copy_status: FileStatus::Untracked,
            working_copy_file_mode: FileMode::Blob,
            path: PathBuf::from(filename),
            orig_path: None,
        }
    }

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
                r#"([a-f\d]+ ){2,3}([CR]\d{1,3} )?"#,                        // HEAD and Index object IDs, and optionally the rename/copy score;
                                                                             // or stage1, stage2, and stage3 object IDs.
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
            path: path.to_vec().into_path_buf()?,
            orig_path: orig_path
                .map(|orig_path| orig_path.to_vec().into_path_buf())
                .transpose()?,
        })
    }
}
