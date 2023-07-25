//! An interactive difftool for use in VCS programs like
//! [Jujutsu](https://github.com/martinvonz/jj) or Git.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::error;
use std::fmt::Display;
use std::fs;
use std::io;
use std::path::{Path, PathBuf, StripPrefixError};

use crate::{File, FileMode, RecordState};
use clap::Parser;
use sha1::Digest;
use walkdir::WalkDir;

#[allow(missing_docs)]
#[derive(Debug)]
pub enum Error {
    Cancelled,
    DryRun,
    WalkDir {
        source: walkdir::Error,
    },
    StripPrefix {
        root: PathBuf,
        path: PathBuf,
        source: StripPrefixError,
    },
    ReadFile {
        path: PathBuf,
        source: io::Error,
    },
    RemoveFile {
        path: PathBuf,
        source: io::Error,
    },
    CopyFile {
        old_path: PathBuf,
        new_path: PathBuf,
        source: io::Error,
    },
    CreateDirAll {
        path: PathBuf,
        source: io::Error,
    },
    WriteFile {
        path: PathBuf,
        source: io::Error,
    },
    MissingMergeFile {
        path: PathBuf,
    },
    BinaryMergeFile {
        path: PathBuf,
    },
    Record {
        source: crate::RecordError,
    },
}

impl error::Error for Error {}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Cancelled => {
                write!(f, "aborted by user")
            }
            Error::DryRun => {
                write!(f, "dry run, not writing any files")
            }
            Error::WalkDir { source } => {
                write!(f, "walking directory: {source}")
            }
            Error::StripPrefix { root, path, source } => {
                write!(
                    f,
                    "stripping directory prefix {} from {}: {source}",
                    root.display(),
                    path.display()
                )
            }
            Error::ReadFile { path, source } => {
                write!(f, "reading file {}: {source}", path.display())
            }
            Error::RemoveFile { path, source } => {
                write!(f, "removing file {}: {source}", path.display())
            }
            Error::CopyFile {
                old_path,
                new_path,
                source,
            } => {
                write!(
                    f,
                    "copying file {} to {}: {source}",
                    old_path.display(),
                    new_path.display()
                )
            }
            Error::CreateDirAll { path, source } => {
                write!(f, "creating directory {}: {source}", path.display())
            }
            Error::WriteFile { path, source } => {
                write!(f, "writing file {}: {source}", path.display())
            }
            Error::MissingMergeFile { path } => {
                write!(f, "file did not exist: {}", path.display())
            }
            Error::BinaryMergeFile { path } => {
                write!(f, "file was not text: {}", path.display())
            }
            Error::Record { source } => {
                write!(f, "recording changes: {source}")
            }
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

/// Information about a file that was read from disk. Note that the file may not have existed, in
/// which case its contents will be marked as absent.
#[derive(Clone, Debug)]
pub struct FileInfo {
    file_mode: FileMode,
    contents: FileContents,
}

#[derive(Clone, Debug)]
enum FileContents {
    Absent,
    Text {
        contents: String,
        hash: String,
        num_bytes: u64,
    },
    Binary {
        hash: String,
        num_bytes: u64,
    },
}

#[allow(missing_docs)]
pub trait Filesystem {
    /// Find the set of files that appear in either `left` or `right`.
    fn read_dir_diff_paths(&self, left: &Path, right: &Path) -> Result<BTreeSet<PathBuf>>;

    fn read_file_info(&self, path: &Path) -> Result<FileInfo>;
    fn write_file(&mut self, path: &Path, contents: &str) -> Result<()>;
    fn copy_file(&mut self, old_path: &Path, new_path: &Path) -> Result<()>;
    fn remove_file(&mut self, path: &Path) -> Result<()>;
    fn create_dir_all(&mut self, path: &Path) -> Result<()>;
}

struct RealFilesystem;

impl Filesystem for RealFilesystem {
    fn read_dir_diff_paths(&self, left: &Path, right: &Path) -> Result<BTreeSet<PathBuf>> {
        fn walk_dir(dir: &Path) -> Result<BTreeSet<PathBuf>> {
            let mut files = BTreeSet::new();
            for entry in WalkDir::new(dir) {
                let entry = entry.map_err(|err| Error::WalkDir { source: err })?;
                if entry.file_type().is_file() || entry.file_type().is_symlink() {
                    let relative_path = match entry.path().strip_prefix(dir) {
                        Ok(path) => path.to_owned(),
                        Err(err) => {
                            return Err(Error::StripPrefix {
                                root: dir.to_owned(),
                                path: entry.path().to_owned(),
                                source: err,
                            })
                        }
                    };
                    files.insert(relative_path);
                }
            }
            Ok(files)
        }
        let left_files = walk_dir(left)?;
        let right_files = walk_dir(right)?;
        let paths = left_files
            .into_iter()
            .chain(right_files.into_iter())
            .collect::<BTreeSet<_>>();
        Ok(paths)
    }

    fn read_file_info(&self, path: &Path) -> Result<FileInfo> {
        let file_mode = match fs::metadata(path) {
            Ok(metadata) => {
                // TODO: no support for gitlinks (submodules).
                if metadata.is_symlink() {
                    FileMode(0o120000)
                } else {
                    let permissions = metadata.permissions();
                    #[cfg(unix)]
                    let executable = {
                        use std::os::unix::fs::PermissionsExt;
                        permissions.mode() & 0o001 == 0o001
                    };
                    #[cfg(not(unix))]
                    let executable = false;
                    if executable {
                        FileMode(0o100755)
                    } else {
                        FileMode(0o100644)
                    }
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => FileMode::absent(),
            Err(err) => {
                return Err(Error::ReadFile {
                    path: path.to_owned(),
                    source: err,
                })
            }
        };
        let contents = match fs::read(path) {
            Ok(contents) => {
                let hash = {
                    let mut hasher = sha1::Sha1::new();
                    hasher.update(&contents);
                    format!("{:x}", hasher.finalize())
                };
                let num_bytes: u64 = contents.len().try_into().unwrap();
                if contents.contains(&0) {
                    FileContents::Binary { hash, num_bytes }
                } else {
                    match String::from_utf8(contents) {
                        Ok(contents) => FileContents::Text {
                            contents,
                            hash,
                            num_bytes,
                        },
                        Err(_) => FileContents::Binary { hash, num_bytes },
                    }
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => FileContents::Absent,
            Err(err) => {
                return Err(Error::ReadFile {
                    path: path.to_owned(),
                    source: err,
                })
            }
        };
        Ok(FileInfo {
            file_mode,
            contents,
        })
    }

    fn write_file(&mut self, path: &Path, contents: &str) -> Result<()> {
        fs::write(path, contents).map_err(|err| Error::WriteFile {
            path: path.to_owned(),
            source: err,
        })
    }

    fn copy_file(&mut self, old_path: &Path, new_path: &Path) -> Result<()> {
        fs::copy(old_path, new_path).map_err(|err| Error::CopyFile {
            old_path: old_path.to_owned(),
            new_path: new_path.to_owned(),
            source: err,
        })?;
        Ok(())
    }

    fn remove_file(&mut self, path: &Path) -> Result<()> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Error::RemoveFile {
                path: path.to_owned(),
                source: err,
            }),
        }
    }

    fn create_dir_all(&mut self, path: &Path) -> Result<()> {
        fs::create_dir_all(path).map_err(|err| Error::CreateDirAll {
            path: path.to_owned(),
            source: err,
        })?;
        Ok(())
    }
}

mod render {
    use std::borrow::Cow;
    use std::path::PathBuf;

    use crate::helpers::make_binary_description;
    use crate::{ChangeType, File, FileMode, Section, SectionChangedLine};
    use tracing::warn;

    use super::{Error, FileContents, FileInfo, Filesystem};

    fn make_section_changed_lines(
        contents: &str,
        change_type: ChangeType,
    ) -> Vec<SectionChangedLine<'static>> {
        contents
            .split_inclusive('\n')
            .map(|line| SectionChangedLine {
                is_checked: false,
                change_type,
                line: Cow::Owned(line.to_owned()),
            })
            .collect()
    }

    pub fn create_file(
        filesystem: &dyn Filesystem,
        left_path: PathBuf,
        left_display_path: PathBuf,
        right_path: PathBuf,
        right_display_path: PathBuf,
    ) -> Result<File<'static>, Error> {
        let FileInfo {
            file_mode: left_file_mode,
            contents: left_contents,
        } = filesystem.read_file_info(&left_path)?;
        let FileInfo {
            file_mode: right_file_mode,
            contents: right_contents,
        } = filesystem.read_file_info(&right_path)?;
        let mut sections = Vec::new();

        if left_file_mode != right_file_mode
            && left_file_mode != FileMode::absent()
            && right_file_mode != FileMode::absent()
        {
            sections.push(Section::FileMode {
                is_checked: false,
                before: left_file_mode,
                after: right_file_mode,
            });
        }

        match (left_contents, right_contents) {
            (FileContents::Absent, FileContents::Absent) => {}
            (
                FileContents::Absent,
                FileContents::Text {
                    contents,
                    hash: _,
                    num_bytes: _,
                },
            ) => sections.push(Section::Changed {
                lines: make_section_changed_lines(&contents, ChangeType::Added),
            }),

            (FileContents::Absent, FileContents::Binary { hash, num_bytes }) => {
                sections.push(Section::Binary {
                    is_checked: false,
                    old_description: None,
                    new_description: Some(Cow::Owned(make_binary_description(&hash, num_bytes))),
                })
            }

            (
                FileContents::Text {
                    contents,
                    hash: _,
                    num_bytes: _,
                },
                FileContents::Absent,
            ) => sections.push(Section::Changed {
                lines: make_section_changed_lines(&contents, ChangeType::Removed),
            }),

            (
                FileContents::Text {
                    contents: old_contents,
                    hash: _,
                    num_bytes: _,
                },
                FileContents::Text {
                    contents: new_contents,
                    hash: _,
                    num_bytes: _,
                },
            ) => {
                sections.extend(create_diff(&old_contents, &new_contents));
            }

            (
                FileContents::Text {
                    contents: _,
                    hash: old_hash,
                    num_bytes: old_num_bytes,
                }
                | FileContents::Binary {
                    hash: old_hash,
                    num_bytes: old_num_bytes,
                },
                FileContents::Text {
                    contents: _,
                    hash: new_hash,
                    num_bytes: new_num_bytes,
                }
                | FileContents::Binary {
                    hash: new_hash,
                    num_bytes: new_num_bytes,
                },
            ) => sections.push(Section::Binary {
                is_checked: false,
                old_description: Some(Cow::Owned(make_binary_description(
                    &old_hash,
                    old_num_bytes,
                ))),
                new_description: Some(Cow::Owned(make_binary_description(
                    &new_hash,
                    new_num_bytes,
                ))),
            }),

            (FileContents::Binary { hash, num_bytes }, FileContents::Absent) => {
                sections.push(Section::Binary {
                    is_checked: false,
                    old_description: Some(Cow::Owned(make_binary_description(&hash, num_bytes))),
                    new_description: None,
                })
            }
        }

        Ok(File {
            old_path: if left_display_path != right_display_path {
                Some(Cow::Owned(left_display_path))
            } else {
                None
            },
            path: Cow::Owned(right_display_path),
            file_mode: None, // TODO
            sections,
        })
    }

    pub fn create_merge_file(
        filesystem: &dyn Filesystem,
        base_path: PathBuf,
        left_path: PathBuf,
        right_path: PathBuf,
        output_path: PathBuf,
    ) -> Result<File<'static>, Error> {
        let FileInfo {
            file_mode: _,
            contents: left_contents,
        } = filesystem.read_file_info(&left_path)?;
        let FileInfo {
            file_mode: _,
            contents: right_contents,
        } = filesystem.read_file_info(&right_path)?;
        let FileInfo {
            file_mode: _,
            contents: base_contents,
        } = filesystem.read_file_info(&base_path)?;

        let (base_contents, left_contents, right_contents) =
            match (base_contents, left_contents, right_contents) {
                (FileContents::Absent, _, _) => {
                    return Err(Error::MissingMergeFile { path: base_path })
                }
                (_, FileContents::Absent, _) => {
                    return Err(Error::MissingMergeFile { path: left_path })
                }
                (_, _, FileContents::Absent) => {
                    return Err(Error::MissingMergeFile { path: right_path })
                }
                (FileContents::Binary { .. }, _, _) => {
                    return Err(Error::BinaryMergeFile { path: base_path })
                }
                (_, FileContents::Binary { .. }, _) => {
                    return Err(Error::BinaryMergeFile { path: left_path })
                }
                (_, _, FileContents::Binary { .. }) => {
                    return Err(Error::BinaryMergeFile { path: right_path })
                }
                (
                    FileContents::Text {
                        contents: base_contents,
                        hash: _,
                        num_bytes: _,
                    },
                    FileContents::Text {
                        contents: left_contents,
                        hash: _,
                        num_bytes: _,
                    },
                    FileContents::Text {
                        contents: right_contents,
                        hash: _,
                        num_bytes: _,
                    },
                ) => (base_contents, left_contents, right_contents),
            };

        let sections = create_merge(&base_contents, &left_contents, &right_contents);
        Ok(File {
            old_path: Some(Cow::Owned(base_path)),
            path: Cow::Owned(output_path),
            file_mode: None,
            sections,
        })
    }

    fn create_diff(old_contents: &str, new_contents: &str) -> Vec<Section<'static>> {
        let patch = {
            // Set the context length to the maximum number of lines in either file,
            // because we will handle abbreviating context ourselves.
            let max_lines = old_contents
                .lines()
                .count()
                .max(new_contents.lines().count());
            let mut diff_options = diffy::DiffOptions::new();
            diff_options.set_context_len(max_lines);
            diff_options.create_patch(old_contents, new_contents)
        };

        let mut sections = Vec::new();
        for hunk in patch.hunks() {
            sections.extend(hunk.lines().iter().fold(Vec::new(), |mut acc, line| {
                match line {
                    diffy::Line::Context(line) => match acc.last_mut() {
                        Some(Section::Unchanged { lines }) => {
                            lines.push(Cow::Owned((*line).to_owned()));
                        }
                        _ => {
                            acc.push(Section::Unchanged {
                                lines: vec![Cow::Owned((*line).to_owned())],
                            });
                        }
                    },
                    diffy::Line::Delete(line) => {
                        let line = SectionChangedLine {
                            is_checked: false,
                            change_type: ChangeType::Removed,
                            line: Cow::Owned((*line).to_owned()),
                        };
                        match acc.last_mut() {
                            Some(Section::Changed { lines }) => {
                                lines.push(line);
                            }
                            _ => {
                                acc.push(Section::Changed { lines: vec![line] });
                            }
                        }
                    }
                    diffy::Line::Insert(line) => {
                        let line = SectionChangedLine {
                            is_checked: false,
                            change_type: ChangeType::Added,
                            line: Cow::Owned((*line).to_owned()),
                        };
                        match acc.last_mut() {
                            Some(Section::Changed { lines }) => {
                                lines.push(line);
                            }
                            _ => {
                                acc.push(Section::Changed { lines: vec![line] });
                            }
                        }
                    }
                }
                acc
            }));
        }
        sections
    }

    fn make_conflict_markers(
        base: &str,
        left: &str,
        right: &str,
    ) -> (String, String, String, String) {
        let all = [base, left, right].concat();
        let left_char = "<";
        let base_start_char = "|";
        let base_end_char = "=";
        let right_char = ">";
        let mut len = 7;
        loop {
            let left_marker = left_char.repeat(len);
            let base_start_marker = base_start_char.repeat(len);
            let base_end_marker = base_end_char.repeat(len);
            let right_marker = right_char.repeat(len);
            if !all.contains(&left_marker)
                && !all.contains(&base_start_marker)
                && !all.contains(&base_end_marker)
                && !all.contains(&right_marker)
            {
                return (
                    left_marker,
                    base_start_marker,
                    base_end_marker,
                    right_marker,
                );
            }
            len += 1;
        }
    }

    fn create_merge(
        base_contents: &str,
        left_contents: &str,
        right_contents: &str,
    ) -> Vec<Section<'static>> {
        let (left_marker, base_start_marker, base_end_marker, right_marker) =
            make_conflict_markers(base_contents, left_contents, right_contents);

        let mut merge_options = diffy::MergeOptions::new();
        merge_options.set_conflict_marker_length(right_marker.len());
        merge_options.set_conflict_style(diffy::ConflictStyle::Diff3);
        let merge = merge_options.merge(base_contents, left_contents, right_contents);
        let conflicted_text = match merge {
            Ok(_) => return Default::default(),
            Err(conflicted_text) => conflicted_text,
        };

        enum MarkerType {
            Left,
            BaseStart,
            BaseEnd,
            Right,
        }
        #[derive(Debug)]
        enum State<'a> {
            Empty,
            Unchanged {
                lines: Vec<Cow<'a, str>>,
            },
            Left {
                left_lines: Vec<Cow<'a, str>>,
            },
            Base {
                left_lines: Vec<Cow<'a, str>>,
                base_lines: Vec<Cow<'a, str>>,
            },
            Right {
                left_lines: Vec<Cow<'a, str>>,
                base_lines: Vec<Cow<'a, str>>,
                right_lines: Vec<Cow<'a, str>>,
            },
        }

        let mut sections = vec![];
        let mut state = State::Empty;
        for line in conflicted_text.split_inclusive('\n') {
            let marker_type = if line.starts_with(&left_marker) {
                Some(MarkerType::Left)
            } else if line.starts_with(&base_start_marker) {
                Some(MarkerType::BaseStart)
            } else if line.starts_with(&base_end_marker) {
                Some(MarkerType::BaseEnd)
            } else if line.starts_with(&right_marker) {
                Some(MarkerType::Right)
            } else {
                None
            };

            let line = Cow::Owned(line.to_owned());
            let (new_state, new_section) = match (state, marker_type) {
                (State::Empty, Some(MarkerType::Left)) => {
                    let new_state = State::Left {
                        left_lines: Default::default(),
                    };
                    (new_state, None)
                }
                (State::Empty, _) => {
                    let new_state = State::Unchanged { lines: vec![line] };
                    (new_state, None)
                }

                (State::Unchanged { lines }, Some(MarkerType::Left)) => {
                    let new_state = State::Left {
                        left_lines: Default::default(),
                    };
                    let new_section = Section::Unchanged { lines };
                    (new_state, Some(new_section))
                }
                (State::Unchanged { mut lines }, _) => {
                    lines.push(line);
                    let new_state = State::Unchanged { lines };
                    (new_state, None)
                }

                (State::Left { left_lines }, Some(MarkerType::BaseStart)) => {
                    let new_state = State::Base {
                        left_lines,
                        base_lines: Default::default(),
                    };
                    (new_state, None)
                }
                (State::Left { mut left_lines }, _) => {
                    left_lines.push(line);
                    let new_state = State::Left { left_lines };
                    (new_state, None)
                }

                (
                    State::Base {
                        left_lines,
                        base_lines,
                    },
                    Some(MarkerType::BaseEnd),
                ) => {
                    let new_state = State::Right {
                        left_lines,
                        base_lines,
                        right_lines: Default::default(),
                    };
                    (new_state, None)
                }
                (
                    State::Base {
                        left_lines,
                        mut base_lines,
                    },
                    _,
                ) => {
                    base_lines.push(line);
                    let new_state = State::Base {
                        left_lines,
                        base_lines,
                    };
                    (new_state, None)
                }

                (
                    State::Right {
                        left_lines,
                        base_lines,
                        right_lines,
                    },
                    Some(MarkerType::Right),
                ) => {
                    let new_state = State::Empty;
                    let new_section = Section::Changed {
                        lines: left_lines
                            .into_iter()
                            .map(|line| (line, ChangeType::Added))
                            .chain(
                                base_lines
                                    .into_iter()
                                    .map(|line| (line, ChangeType::Removed)),
                            )
                            .chain(
                                right_lines
                                    .into_iter()
                                    .map(|line| (line, ChangeType::Added)),
                            )
                            .map(|(line, change_type)| SectionChangedLine {
                                is_checked: false,
                                change_type,
                                line,
                            })
                            .collect(),
                    };
                    (new_state, Some(new_section))
                }
                (
                    State::Right {
                        left_lines,
                        base_lines,
                        mut right_lines,
                    },
                    _,
                ) => {
                    right_lines.push(line);
                    let new_state = State::Right {
                        left_lines,
                        base_lines,
                        right_lines,
                    };
                    (new_state, None)
                }
            };

            state = new_state;
            if let Some(new_section) = new_section {
                sections.push(new_section);
            }
        }

        match state {
            State::Empty => {}
            State::Unchanged { lines } => {
                sections.push(Section::Unchanged { lines });
            }
            state @ (State::Left { .. } | State::Base { .. } | State::Right { .. }) => {
                warn!(?state, "Diff section not terminated");
            }
        }

        sections
    }
}

fn print_dry_run(write_root: &Path, state: RecordState) {
    let crate::RecordState {
        is_read_only: _,
        files,
    } = state;
    for file in files {
        let file_path = write_root.join(file.path.clone());
        let (selected_contents, _unselected_contents) = file.get_selected_contents();
        match selected_contents {
            crate::SelectedContents::Absent => {
                println!("Would delete file: {}", file_path.display())
            }
            crate::SelectedContents::Unchanged => {
                println!("Would leave file unchanged: {}", file_path.display())
            }
            crate::SelectedContents::Binary {
                old_description,
                new_description,
            } => {
                println!("Would update binary file: {}", file_path.display());
                println!("  Old: {:?}", old_description);
                println!("  New: {:?}", new_description);
            }
            crate::SelectedContents::Present { contents } => {
                println!("Would update text file: {}", file_path.display());
                for line in contents.lines() {
                    println!("  {line}");
                }
            }
        }
    }
}

fn apply_changes(
    filesystem: &mut dyn Filesystem,
    write_root: &Path,
    state: RecordState,
) -> Result<()> {
    let crate::RecordState {
        is_read_only,
        files,
    } = state;
    if is_read_only {
        return Ok(());
    }
    for file in files {
        let file_path = write_root.join(file.path.clone());
        let (selected_contents, _unselected_contents) = file.get_selected_contents();
        match selected_contents {
            crate::SelectedContents::Absent => {
                filesystem.remove_file(&file_path)?;
            }
            crate::SelectedContents::Unchanged => {
                // Do nothing.
            }
            crate::SelectedContents::Binary {
                old_description: _,
                new_description: _,
            } => {
                let new_path = file_path;
                let old_path = match &file.old_path {
                    Some(old_path) => old_path.clone(),
                    None => Cow::Borrowed(new_path.as_path()),
                };
                filesystem.copy_file(&old_path, &new_path)?;
            }
            crate::SelectedContents::Present { contents } => {
                if let Some(parent_dir) = file_path.parent() {
                    filesystem.create_dir_all(parent_dir)?;
                }
                filesystem.write_file(&file_path, &contents)?;
            }
        }
    }
    Ok(())
}

/// Render a partial commit selector for use as a difftool or mergetool.
///
/// This can be used to interactively select changes to include as part of a
/// commit, to resolve merge conflicts, or to simply display a diff in a
/// readable way.
#[derive(Debug, Parser)]
pub struct Opts {
    /// Instead of comparing two files, compare two directories recursively.
    #[clap(short = 'd', long = "dir-diff")]
    dir_diff: bool,

    /// The left-hand file to compare (or directory if `--dir-diff` is passed).
    left: PathBuf,

    /// The right-hand file to compare (or directory if `--dir-diff` is passed).
    right: PathBuf,

    /// Disable all editing controls and do not write the selected commit
    /// contents to disk.
    #[clap(long = "read-only")]
    read_only: bool,

    /// Show what would have been written to disk as part of the commit
    /// selection, but do not actually write it.
    #[clap(short = 'N', long = "dry-run")]
    dry_run: bool,

    /// Render the interface as a mergetool instead of a difftool and use this
    /// file as the base of a three-way diff as part of resolving merge
    /// conflicts.
    #[clap(
        short = 'b',
        long = "base",
        requires("output"),
        conflicts_with("dir_diff")
    )]
    base: Option<PathBuf>,

    /// Write the resolved merge conflicts to this file.
    #[clap(short = 'o', long = "output", conflicts_with("dir_diff"))]
    output: Option<PathBuf>,
}

fn process_opts(filesystem: &dyn Filesystem, opts: &Opts) -> Result<(Vec<File<'static>>, PathBuf)> {
    let result = match opts {
        Opts {
            dir_diff: false,
            left,
            right,
            base: None,
            output: _,
            read_only: _,
            dry_run: _,
        } => {
            let files = vec![render::create_file(
                filesystem,
                left.clone(),
                left.clone(),
                right.clone(),
                right.clone(),
            )?];
            (files, PathBuf::new())
        }

        Opts {
            dir_diff: true,
            left,
            right,
            base: None,
            output: _,
            read_only: _,
            dry_run: _,
        } => {
            let display_paths = filesystem.read_dir_diff_paths(left, right)?;
            let mut files = Vec::new();
            for display_path in display_paths {
                files.push(render::create_file(
                    filesystem,
                    left.join(&display_path),
                    display_path.clone(),
                    right.join(&display_path),
                    display_path.clone(),
                )?);
            }
            (files, right.clone())
        }

        Opts {
            dir_diff: false,
            left,
            right,
            base: Some(base),
            output: Some(output),
            read_only: _,
            dry_run: _,
        } => {
            let files = vec![render::create_merge_file(
                filesystem,
                base.clone(),
                left.clone(),
                right.clone(),
                output.clone(),
            )?];
            (files, PathBuf::new())
        }

        Opts {
            dir_diff: false,
            left: _,
            right: _,
            base: Some(_),
            output: None,
            read_only: _,
            dry_run: _,
        } => {
            unreachable!("--output is required when --base is provided");
        }

        Opts {
            dir_diff: true,
            left: _,
            right: _,
            base: Some(_),
            output: _,
            read_only: _,
            dry_run: _,
        } => {
            unimplemented!("--base cannot be used with --dir-diff");
        }
    };
    Ok(result)
}

/// Launch the terminal user interface (TUI) tool to view/edit diffs/merge
/// conflicts.
pub fn scm_diff_editor_main(opts: Opts) -> Result<()> {
    let filesystem = RealFilesystem;
    let (files, write_root) = process_opts(&filesystem, &opts)?;
    let state = crate::RecordState {
        is_read_only: opts.read_only,
        files,
    };
    let event_source = crate::EventSource::Crossterm;
    let recorder = crate::Recorder::new(state, event_source);
    match recorder.run() {
        Ok(state) => {
            if opts.dry_run {
                print_dry_run(&write_root, state);
                Err(Error::DryRun)
            } else {
                let mut filesystem = filesystem;
                apply_changes(&mut filesystem, &write_root, state)?;
                Ok(())
            }
        }
        Err(crate::RecordError::Cancelled) => Err(Error::Cancelled),
        Err(err) => Err(Error::Record { source: err }),
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_debug_snapshot;
    use maplit::btreemap;
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Debug)]
    struct TestFilesystem {
        files: BTreeMap<PathBuf, FileInfo>,
        dirs: BTreeSet<PathBuf>,
    }

    impl TestFilesystem {
        pub fn new(files: BTreeMap<PathBuf, FileInfo>) -> Self {
            let dirs = files
                .keys()
                .flat_map(|path| path.ancestors().skip(1))
                .map(|path| path.to_owned())
                .collect();
            Self { files, dirs }
        }

        fn assert_parent_dir_exists(&self, path: &Path) {
            if let Some(parent_dir) = path.parent() {
                assert!(
                    self.dirs.contains(parent_dir),
                    "parent dir for {path:?} does not exist"
                );
            }
        }
    }

    impl Filesystem for TestFilesystem {
        fn read_dir_diff_paths(&self, left: &Path, right: &Path) -> Result<BTreeSet<PathBuf>> {
            Ok(self
                .files
                .keys()
                .filter(|path| path.starts_with(left) || path.starts_with(right))
                .cloned()
                .collect())
        }

        fn read_file_info(&self, path: &Path) -> Result<FileInfo> {
            match self.files.get(path) {
                Some(file_info) => Ok(file_info.clone()),
                None => match self.dirs.get(path) {
                    Some(_path) => Err(Error::ReadFile {
                        path: path.to_owned(),
                        source: io::Error::new(io::ErrorKind::Other, "is a directory"),
                    }),
                    None => Ok(FileInfo {
                        file_mode: FileMode::absent(),
                        contents: FileContents::Absent,
                    }),
                },
            }
        }

        fn write_file(&mut self, path: &Path, contents: &str) -> Result<()> {
            self.assert_parent_dir_exists(path);
            self.files.insert(path.to_owned(), file_info(contents));
            Ok(())
        }

        fn copy_file(&mut self, old_path: &Path, new_path: &Path) -> Result<()> {
            self.assert_parent_dir_exists(new_path);
            let file_info = self.read_file_info(old_path)?;
            self.files.insert(new_path.to_owned(), file_info);
            Ok(())
        }

        fn remove_file(&mut self, path: &Path) -> Result<()> {
            match self.files.remove(path) {
                Some(_) => Ok(()),
                None => {
                    panic!("tried to remove non-existent file: {path:?}");
                }
            }
        }

        fn create_dir_all(&mut self, path: &Path) -> Result<()> {
            self.dirs.insert(path.to_owned());
            Ok(())
        }
    }

    fn file_info(contents: impl Into<String>) -> FileInfo {
        let contents = contents.into();
        let num_bytes = contents.len().try_into().unwrap();
        FileInfo {
            file_mode: FileMode(0o100644),
            contents: FileContents::Text {
                contents,
                hash: "abc123".to_string(),
                num_bytes,
            },
        }
    }

    fn select_all(files: &mut [File]) {
        for file in files {
            file.set_checked(true);
        }
    }

    #[test]
    fn test_diff() -> Result<()> {
        let mut filesystem = TestFilesystem::new(btreemap! {
            PathBuf::from("left") => file_info("\
foo
common1
common2
bar
"),
            PathBuf::from("right") => file_info("\
qux1
common1
common2
qux2
"),
        });
        let (mut files, write_root) = process_opts(
            &filesystem,
            &Opts {
                dir_diff: false,
                left: PathBuf::from("left"),
                right: PathBuf::from("right"),
                base: None,
                output: None,
                read_only: false,
                dry_run: false,
            },
        )?;
        assert_debug_snapshot!(files, @r###"
        [
            File {
                old_path: Some(
                    "left",
                ),
                path: "right",
                file_mode: None,
                sections: [
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Removed,
                                line: "foo\n",
                            },
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "qux1\n",
                            },
                        ],
                    },
                    Unchanged {
                        lines: [
                            "common1\n",
                            "common2\n",
                        ],
                    },
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Removed,
                                line: "bar\n",
                            },
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "qux2\n",
                            },
                        ],
                    },
                ],
            },
        ]
        "###);

        select_all(&mut files);
        apply_changes(
            &mut filesystem,
            &write_root,
            RecordState {
                is_read_only: false,
                files,
            },
        )?;
        insta::assert_debug_snapshot!(filesystem, @r###"
        TestFilesystem {
            files: {
                "left": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "foo\ncommon1\ncommon2\nbar\n",
                        hash: "abc123",
                        num_bytes: 24,
                    },
                },
                "right": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "qux1\ncommon1\ncommon2\nqux2\n",
                        hash: "abc123",
                        num_bytes: 26,
                    },
                },
            },
            dirs: {
                "",
            },
        }
        "###);

        Ok(())
    }

    #[test]
    fn test_diff_no_changes() -> Result<()> {
        let mut filesystem = TestFilesystem::new(btreemap! {
            PathBuf::from("left") => file_info("\
foo
common1
common2
bar
"),
            PathBuf::from("right") => file_info("\
qux1
common1
common2
qux2
"),
        });
        let (files, write_root) = process_opts(
            &filesystem,
            &Opts {
                dir_diff: false,
                left: PathBuf::from("left"),
                right: PathBuf::from("right"),
                base: None,
                output: None,
                read_only: false,
                dry_run: false,
            },
        )?;

        apply_changes(
            &mut filesystem,
            &write_root,
            RecordState {
                is_read_only: false,
                files,
            },
        )?;
        insta::assert_debug_snapshot!(filesystem, @r###"
        TestFilesystem {
            files: {
                "left": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "foo\ncommon1\ncommon2\nbar\n",
                        hash: "abc123",
                        num_bytes: 24,
                    },
                },
                "right": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "foo\ncommon1\ncommon2\nbar\n",
                        hash: "abc123",
                        num_bytes: 24,
                    },
                },
            },
            dirs: {
                "",
            },
        }
        "###);

        Ok(())
    }

    #[test]
    fn test_diff_absent_left() -> Result<()> {
        let mut filesystem = TestFilesystem::new(btreemap! {
            PathBuf::from("right") => file_info("right\n"),
        });
        let (mut files, write_root) = process_opts(
            &filesystem,
            &Opts {
                dir_diff: false,
                left: PathBuf::from("left"),
                right: PathBuf::from("right"),
                base: None,
                output: None,
                read_only: false,
                dry_run: false,
            },
        )?;
        assert_debug_snapshot!(files, @r###"
        [
            File {
                old_path: Some(
                    "left",
                ),
                path: "right",
                file_mode: None,
                sections: [
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "right\n",
                            },
                        ],
                    },
                ],
            },
        ]
        "###);

        select_all(&mut files);
        apply_changes(
            &mut filesystem,
            &write_root,
            RecordState {
                is_read_only: false,
                files,
            },
        )?;
        insta::assert_debug_snapshot!(filesystem, @r###"
        TestFilesystem {
            files: {
                "right": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "right\n",
                        hash: "abc123",
                        num_bytes: 6,
                    },
                },
            },
            dirs: {
                "",
            },
        }
        "###);

        Ok(())
    }

    #[test]
    fn test_diff_absent_right() -> Result<()> {
        let mut filesystem = TestFilesystem::new(btreemap! {
            PathBuf::from("left") => file_info("left\n"),
        });
        let (mut files, write_root) = process_opts(
            &filesystem,
            &Opts {
                dir_diff: false,
                left: PathBuf::from("left"),
                right: PathBuf::from("right"),
                base: None,
                output: None,
                read_only: false,
                dry_run: false,
            },
        )?;
        assert_debug_snapshot!(files, @r###"
        [
            File {
                old_path: Some(
                    "left",
                ),
                path: "right",
                file_mode: None,
                sections: [
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Removed,
                                line: "left\n",
                            },
                        ],
                    },
                ],
            },
        ]
        "###);

        select_all(&mut files);
        apply_changes(
            &mut filesystem,
            &write_root,
            RecordState {
                is_read_only: false,
                files,
            },
        )?;
        insta::assert_debug_snapshot!(filesystem, @r###"
        TestFilesystem {
            files: {
                "left": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "left\n",
                        hash: "abc123",
                        num_bytes: 5,
                    },
                },
            },
            dirs: {
                "",
            },
        }
        "###);

        Ok(())
    }

    #[test]
    fn test_reject_diff_non_files() -> Result<()> {
        let filesystem = TestFilesystem::new(btreemap! {
            PathBuf::from("left/foo") => file_info("left\n"),
            PathBuf::from("right/foo") => file_info("right\n"),
        });
        let result = process_opts(
            &filesystem,
            &Opts {
                dir_diff: false,
                left: PathBuf::from("left"),
                right: PathBuf::from("right"),
                base: None,
                output: None,
                read_only: false,
                dry_run: false,
            },
        );
        insta::assert_debug_snapshot!(result, @r###"
        Err(
            ReadFile {
                path: "left",
                source: Custom {
                    kind: Other,
                    error: "is a directory",
                },
            },
        )
        "###);

        Ok(())
    }

    #[test]
    fn test_diff_files_in_subdirectories() -> Result<()> {
        let mut filesystem = TestFilesystem::new(btreemap! {
            PathBuf::from("left/foo") => file_info("left contents\n"),
            PathBuf::from("right/foo") => file_info("right contents\n"),
        });

        let (files, write_root) = process_opts(
            &filesystem,
            &Opts {
                dir_diff: false,
                left: PathBuf::from("left/foo"),
                right: PathBuf::from("right/foo"),
                base: None,
                output: None,
                read_only: false,
                dry_run: false,
            },
        )?;

        apply_changes(
            &mut filesystem,
            &write_root,
            RecordState {
                is_read_only: false,
                files,
            },
        )?;
        assert_debug_snapshot!(filesystem, @r###"
        TestFilesystem {
            files: {
                "left/foo": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "left contents\n",
                        hash: "abc123",
                        num_bytes: 14,
                    },
                },
                "right/foo": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "left contents\n",
                        hash: "abc123",
                        num_bytes: 14,
                    },
                },
            },
            dirs: {
                "",
                "left",
                "right",
            },
        }
        "###);

        Ok(())
    }

    #[test]
    fn test_dir_diff_no_changes() -> Result<()> {
        let mut filesystem = TestFilesystem::new(btreemap! {
            PathBuf::from("left/foo") => file_info("left contents\n"),
            PathBuf::from("right/foo") => file_info("right contents\n"),
        });

        let (files, write_root) = process_opts(
            &filesystem,
            &Opts {
                dir_diff: false,
                left: PathBuf::from("left/foo"),
                right: PathBuf::from("right/foo"),
                base: None,
                output: None,
                read_only: false,
                dry_run: false,
            },
        )?;

        apply_changes(
            &mut filesystem,
            &write_root,
            RecordState {
                is_read_only: false,
                files,
            },
        )?;
        assert_debug_snapshot!(filesystem, @r###"
        TestFilesystem {
            files: {
                "left/foo": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "left contents\n",
                        hash: "abc123",
                        num_bytes: 14,
                    },
                },
                "right/foo": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "left contents\n",
                        hash: "abc123",
                        num_bytes: 14,
                    },
                },
            },
            dirs: {
                "",
                "left",
                "right",
            },
        }
        "###);

        Ok(())
    }

    #[test]
    fn test_create_merge() -> Result<()> {
        let base_contents = "\
Hello world 1
Hello world 2
Hello world 3
Hello world 4
";
        let left_contents = "\
Hello world 1
Hello world 2
Hello world L
Hello world 4
";
        let right_contents = "\
Hello world 1
Hello world 2
Hello world R
Hello world 4
";
        let mut filesystem = TestFilesystem::new(btreemap! {
            PathBuf::from("base") => file_info(base_contents),
            PathBuf::from("left") => file_info(left_contents),
            PathBuf::from("right") => file_info(right_contents),
        });

        let (mut files, write_root) = process_opts(
            &filesystem,
            &Opts {
                dir_diff: false,
                left: "left".into(),
                right: "right".into(),
                read_only: false,
                dry_run: false,
                base: Some("base".into()),
                output: Some("output".into()),
            },
        )?;
        insta::assert_debug_snapshot!(files, @r###"
        [
            File {
                old_path: Some(
                    "base",
                ),
                path: "output",
                file_mode: None,
                sections: [
                    Unchanged {
                        lines: [
                            "Hello world 1\n",
                            "Hello world 2\n",
                        ],
                    },
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "Hello world L\n",
                            },
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Removed,
                                line: "Hello world 3\n",
                            },
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "Hello world R\n",
                            },
                        ],
                    },
                    Unchanged {
                        lines: [
                            "Hello world 4\n",
                        ],
                    },
                ],
            },
        ]
        "###);

        select_all(&mut files);
        apply_changes(
            &mut filesystem,
            &write_root,
            RecordState {
                is_read_only: false,
                files,
            },
        )?;

        assert_debug_snapshot!(filesystem, @r###"
        TestFilesystem {
            files: {
                "base": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "Hello world 1\nHello world 2\nHello world 3\nHello world 4\n",
                        hash: "abc123",
                        num_bytes: 56,
                    },
                },
                "left": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "Hello world 1\nHello world 2\nHello world L\nHello world 4\n",
                        hash: "abc123",
                        num_bytes: 56,
                    },
                },
                "output": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "Hello world 1\nHello world 2\nHello world L\nHello world R\nHello world 4\n",
                        hash: "abc123",
                        num_bytes: 70,
                    },
                },
                "right": FileInfo {
                    file_mode: FileMode(
                        33188,
                    ),
                    contents: Text {
                        contents: "Hello world 1\nHello world 2\nHello world R\nHello world 4\n",
                        hash: "abc123",
                        num_bytes: 56,
                    },
                },
            },
            dirs: {
                "",
            },
        }
        "###);

        Ok(())
    }
}
