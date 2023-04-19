//! Supporting library for
//! [git-branchless](https://github.com/arxanas/git-branchless).
//!
//! This is a UI component to interactively select changes to include in a
//! commit. It's meant to be embedded in source control tooling.
//!
//! You can think of this as an interactive replacement for `git add -p`, or a
//! reimplementation of `hg crecord`. Given a set of changes made by the user,
//! this component presents them to the user and lets them select which of those
//! changes should be staged for commit.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::error;
use std::fmt::Display;
use std::fs;
use std::io;
use std::path::{Path, PathBuf, StripPrefixError};

use clap::Parser;
use scm_record::RecordState;
use scm_record::{
    helpers::make_binary_description, ChangeType, File, FileMode, Section, SectionChangedLine,
};
use sha1::Digest;
use walkdir::WalkDir;

#[derive(Debug)]
enum Error {
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
        parent_dir: PathBuf,
        path: PathBuf,
        source: io::Error,
    },
    WriteFile {
        path: PathBuf,
        source: io::Error,
    },
    Record {
        source: scm_record::RecordError,
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
            Error::CreateDirAll {
                parent_dir,
                path,
                source,
            } => {
                write!(
                    f,
                    "creating parent directory {} for path {}: {source}",
                    parent_dir.display(),
                    path.display(),
                )
            }
            Error::WriteFile { path, source } => {
                write!(f, "writing file {}: {source}", path.display())
            }
            Error::Record { source } => {
                write!(f, "recording changes: {source}")
            }
        }
    }
}

#[derive(Debug, Parser)]
struct Opts {
    #[clap(short = 'd', long = "dir-diff")]
    dir_diff: bool,
    left: PathBuf,
    right: PathBuf,
    #[clap(short = 'N', long = "dry-run")]
    dry_run: bool,
}

struct FileInfo {
    file_mode: FileMode,
    contents: FileContents,
}

#[derive(Debug)]
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

fn make_section_changed_lines(
    contents: &str,
    change_type: ChangeType,
) -> Vec<SectionChangedLine<'static>> {
    contents
        .lines()
        .map(|line| SectionChangedLine {
            is_toggled: false,
            change_type,
            line: Cow::Owned(line.to_owned()),
        })
        .collect()
}

fn read_file_info(path: PathBuf) -> Result<FileInfo, Error> {
    let file_mode = match fs::metadata(&path) {
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
        Err(err) => return Err(Error::ReadFile { path, source: err }),
    };
    let contents = match fs::read(&path) {
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
        Err(err) => return Err(Error::ReadFile { path, source: err }),
    };
    Ok(FileInfo {
        file_mode,
        contents,
    })
}

fn create_file(
    left_path: PathBuf,
    left_display_path: PathBuf,
    right_path: PathBuf,
    right_display_path: PathBuf,
) -> Result<File<'static>, Error> {
    let FileInfo {
        file_mode: left_file_mode,
        contents: left_contents,
    } = read_file_info(left_path)?;
    let FileInfo {
        file_mode: right_file_mode,
        contents: right_contents,
    } = read_file_info(right_path)?;
    let mut sections = Vec::new();

    if left_file_mode != right_file_mode
        && left_file_mode != FileMode::absent()
        && right_file_mode != FileMode::absent()
    {
        sections.push(Section::FileMode {
            is_toggled: false,
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
                is_toggled: false,
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
            is_toggled: false,
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
                is_toggled: false,
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
                        is_toggled: false,
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
                        is_toggled: false,
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

fn print_dry_run(write_root: &Path, state: RecordState) {
    let scm_record::RecordState { files } = state;
    for file in files {
        let file_path = write_root.join(file.path.clone());
        let (selected_contents, _unselected_contents) = file.get_selected_contents();
        match selected_contents {
            scm_record::SelectedContents::Absent => {
                println!("Would delete file: {}", file_path.display())
            }
            scm_record::SelectedContents::Unchanged => {
                println!("Would leave file unchanged: {}", file_path.display())
            }
            scm_record::SelectedContents::Binary {
                old_description,
                new_description,
            } => {
                println!("Would update binary file: {}", file_path.display());
                println!("  Old: {:?}", old_description);
                println!("  New: {:?}", new_description);
            }
            scm_record::SelectedContents::Present { contents } => {
                println!("Would update text file: {}", file_path.display());
                for line in contents.lines() {
                    println!("  {line}");
                }
            }
        }
    }
}

fn apply_changes(write_root: &Path, state: RecordState) -> Result<(), Error> {
    let scm_record::RecordState { files } = state;
    for file in files {
        let file_path = write_root.join(file.path.clone());
        let (selected_contents, _unselected_contents) = file.get_selected_contents();
        match selected_contents {
            scm_record::SelectedContents::Absent => match fs::remove_file(&file_path) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(Error::RemoveFile {
                        path: file_path,
                        source: err,
                    });
                }
            },
            scm_record::SelectedContents::Unchanged => {
                // Do nothing.
            }
            scm_record::SelectedContents::Binary {
                old_description: _,
                new_description: _,
            } => {
                let new_path = file_path;
                let old_path = match &file.old_path {
                    Some(old_path) => old_path.clone(),
                    None => Cow::Borrowed(new_path.as_path()),
                };
                match fs::copy(&old_path, &new_path) {
                    Ok(_bytes_written) => {}
                    Err(err) => {
                        return Err(Error::CopyFile {
                            source: err,
                            old_path: old_path.clone().into_owned(),
                            new_path,
                        });
                    }
                };
            }
            scm_record::SelectedContents::Present { contents } => {
                if let Some(parent_dir) = file_path.parent() {
                    fs::create_dir_all(parent_dir).map_err(|err| Error::CreateDirAll {
                        parent_dir: parent_dir.to_owned(),
                        path: file_path.clone(),
                        source: err,
                    })?;
                }
                match fs::write(&file_path, contents) {
                    Ok(()) => {}
                    Err(err) => {
                        return Err(Error::WriteFile {
                            path: file_path,
                            source: err,
                        })
                    }
                }
            }
        }
    }
    Ok(())
}

fn main_inner() -> Result<(), Error> {
    let opts = Opts::parse();
    let (files, write_root) = match opts {
        Opts {
            dir_diff: false,
            left,
            right,
            dry_run: _,
        } => {
            let write_root = right
                .parent()
                .map(|path| path.to_owned())
                .unwrap_or_default();
            let files = vec![create_file(left.clone(), left, right.clone(), right)?];
            (files, write_root)
        }
        Opts {
            dir_diff: true,
            left,
            right,
            dry_run: _,
        } => {
            fn walk_dir(dir: &Path) -> Result<BTreeMap<PathBuf, PathBuf>, Error> {
                let mut files = BTreeMap::new();
                for entry in WalkDir::new(dir) {
                    let entry = entry.map_err(|err| Error::WalkDir { source: err })?;
                    if entry.file_type().is_file() || entry.file_type().is_symlink() {
                        let display_path = match entry.path().strip_prefix(dir) {
                            Ok(path) => path.to_owned(),
                            Err(err) => {
                                return Err(Error::StripPrefix {
                                    root: dir.to_owned(),
                                    path: entry.path().to_owned(),
                                    source: err,
                                })
                            }
                        };
                        files.insert(display_path, entry.path().to_owned());
                    }
                }
                Ok(files)
            }
            let left_files = walk_dir(&left)?;
            let right_files = walk_dir(&right)?;
            let display_paths = left_files
                .keys()
                .chain(right_files.keys())
                .collect::<BTreeSet<_>>();
            let mut files = Vec::new();
            for display_path in display_paths {
                files.push(create_file(
                    left.join(display_path),
                    display_path.clone(),
                    right.join(display_path),
                    display_path.clone(),
                )?);
            }
            (files, right)
        }
    };

    let state = scm_record::RecordState { files };
    let event_source = scm_record::EventSource::Crossterm;
    let recorder = scm_record::Recorder::new(state, event_source);
    match recorder.run() {
        Ok(state) => {
            if opts.dry_run {
                print_dry_run(&write_root, state);
                Err(Error::DryRun)
            } else {
                apply_changes(&write_root, state)?;
                Ok(())
            }
        }
        Err(scm_record::RecordError::Cancelled) => Err(Error::Cancelled),
        Err(err) => Err(Error::Record { source: err }),
    }
}

fn main() {
    match main_inner() {
        Ok(()) => {}
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}
