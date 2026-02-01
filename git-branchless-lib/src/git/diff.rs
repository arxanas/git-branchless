use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eyre::{Context, OptionExt};
use itertools::Itertools;
use scm_record::helpers::make_binary_description;
use scm_record::{ChangeType, File, FileMode, Section, SectionChangedLine};

use super::{MaybeZeroOid, Repo};

/// A diff between two trees/commits.
pub struct Diff<'repo> {
    pub(super) inner: git2::Diff<'repo>,
}

impl Diff<'_> {
    /// Summarize this diff into a single line "short" format.
    pub fn short_stats(&self) -> eyre::Result<String> {
        let stats = self.inner.stats()?;
        let buf = stats.to_buf(git2::DiffStatsFormat::SHORT, usize::MAX)?;
        buf.as_str()
            .ok_or_eyre("converting buf to str")
            .map(|s| s.trim().to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct GitHunk {
    old_start: usize,
    old_lines: usize,
    new_start: usize,
    new_lines: usize,
}

/// Summarize a diff for use as part of a temporary commit message.
pub fn summarize_diff_for_temporary_commit(diff: &Diff) -> eyre::Result<String> {
    // this returns something like `1 file changed, 1 deletion(-)`
    // diff.short_stats()

    // this builds something like `test2.txt (-1)` or `2 files (+1/-2)`
    let stats = diff.inner.stats()?;
    let filename_or_count = if stats.files_changed() == 1 {
        let mut filename = None;

        // returning false in the closure terminates iteration, but that also
        // returns an Err, so catch and ignore it
        let _ = diff.inner.foreach(
            &mut |delta: git2::DiffDelta, _| {
                let relevant_path = delta
                    .old_file()
                    .path()
                    .or(delta.new_file().path())
                    .unwrap_or_else(|| unreachable!("diff should have contained at least 1 file"));
                filename = Some(format!("{}", relevant_path.display()));
                false
            },
            None,
            None,
            None,
        );

        filename.unwrap_or_else(|| unreachable!("file name should have been initialized"))
    } else {
        format!("{} files", stats.files_changed())
    };

    let ins_del = match (stats.insertions(), stats.deletions()) {
        (0, 0) => unreachable!("empty diff"),
        (i, 0) => format!("+{i}"),
        (0, d) => format!("-{d}"),
        (i, d) => format!("+{i}/-{d}"),
    };

    Ok(format!("{filename_or_count} ({ins_del})"))
}

/// Calculate the diff between the index and the working copy.
pub fn process_diff_for_record(repo: &Repo, diff: &Diff) -> eyre::Result<Vec<File<'static>>> {
    let Diff { inner: diff } = diff;

    #[derive(Clone, Debug)]
    enum DeltaFileContent {
        Hunks(Vec<GitHunk>),
        Binary {
            old_num_bytes: u64,
            new_num_bytes: u64,
        },
    }

    #[derive(Clone, Debug)]
    struct Delta {
        old_oid: git2::Oid,
        old_file_mode: git2::FileMode,
        new_oid: git2::Oid,
        new_file_mode: git2::FileMode,
        content: DeltaFileContent,
    }
    let deltas: Arc<Mutex<HashMap<PathBuf, Delta>>> = Default::default();
    diff.foreach(
        &mut |delta, _| {
            let mut deltas = deltas.lock().unwrap();
            let old_file = delta.old_file().path().unwrap().into();
            let new_file = delta.new_file().path().unwrap().into();
            let delta = Delta {
                old_oid: delta.old_file().id(),
                old_file_mode: delta.old_file().mode(),
                new_oid: delta.new_file().id(),
                new_file_mode: delta.new_file().mode(),
                content: DeltaFileContent::Hunks(Default::default()),
            };
            deltas.insert(old_file, delta.clone());
            deltas.insert(new_file, delta);
            true
        },
        Some(&mut |delta, _| {
            let mut deltas = deltas.lock().unwrap();

            let old_file = delta.old_file().path().unwrap().into();
            let new_file = delta.new_file().path().unwrap().into();
            let delta = Delta {
                old_oid: delta.old_file().id(),
                old_file_mode: delta.old_file().mode(),
                new_oid: delta.new_file().id(),
                new_file_mode: delta.new_file().mode(),
                content: DeltaFileContent::Binary {
                    old_num_bytes: delta.old_file().size(),
                    new_num_bytes: delta.new_file().size(),
                },
            };
            deltas.insert(old_file, delta.clone());
            deltas.insert(new_file, delta);
            true
        }),
        Some(&mut |delta, hunk| {
            let path = delta.new_file().path().unwrap();
            let mut deltas = deltas.lock().unwrap();
            match &mut deltas.get_mut(path).unwrap().content {
                DeltaFileContent::Hunks(hunks) => {
                    hunks.push(GitHunk {
                        old_start: hunk.old_start().try_into().unwrap(),
                        old_lines: hunk.old_lines().try_into().unwrap(),
                        new_start: hunk.new_start().try_into().unwrap(),
                        new_lines: hunk.new_lines().try_into().unwrap(),
                    });
                }
                DeltaFileContent::Binary { .. } => {
                    panic!("File {path:?} got a hunk callback, but it was a binary file")
                }
            }
            true
        }),
        None,
    )
    .wrap_err("Iterating over diff deltas")?;

    let deltas = std::mem::take(&mut *deltas.lock().unwrap());
    let mut result = Vec::new();
    for (path, delta) in deltas {
        let Delta {
            old_oid,
            old_file_mode,
            new_oid,
            new_file_mode,
            content,
        } = delta;
        let old_file_mode = u32::from(old_file_mode);
        let old_file_mode = FileMode::try_from(old_file_mode).unwrap();
        let new_file_mode = u32::from(new_file_mode);
        let new_file_mode = FileMode::try_from(new_file_mode).unwrap();

        if new_oid.is_zero() {
            result.push(File {
                old_path: None,
                path: Cow::Owned(path),
                file_mode: old_file_mode,
                sections: vec![Section::FileMode {
                    is_checked: false,
                    mode: FileMode::Absent,
                }],
            });
            continue;
        }

        let hunks = match content {
            DeltaFileContent::Binary {
                old_num_bytes,
                new_num_bytes,
            } => {
                result.push(File {
                    old_path: None,
                    path: Cow::Owned(path),
                    file_mode: old_file_mode,
                    sections: vec![Section::Binary {
                        is_checked: false,
                        old_description: Some(Cow::Owned(make_binary_description(
                            &old_oid.to_string(),
                            old_num_bytes,
                        ))),
                        new_description: Some(Cow::Owned(make_binary_description(
                            &new_oid.to_string(),
                            new_num_bytes,
                        ))),
                    }],
                });
                continue;
            }
            DeltaFileContent::Hunks(mut hunks) => {
                hunks.sort_by_key(|hunk| (hunk.old_start, hunk.old_lines));
                hunks
            }
        };

        enum BlobContents {
            Absent,
            Binary(u64),
            Text(Vec<String>),
        }
        let get_lines_from_blob = |oid| -> eyre::Result<BlobContents> {
            let oid = MaybeZeroOid::from(oid);
            match oid {
                MaybeZeroOid::Zero => Ok(BlobContents::Absent),
                MaybeZeroOid::NonZero(oid) => {
                    let blob = repo.find_blob_or_fail(oid)?;
                    let num_bytes = blob.size();
                    if blob.is_binary() {
                        return Ok(BlobContents::Binary(num_bytes));
                    }

                    let contents = blob.get_content();
                    let contents = match std::str::from_utf8(contents) {
                        Ok(contents) => contents,
                        Err(_) => {
                            return Ok(BlobContents::Binary(num_bytes));
                        }
                    };

                    let lines: Vec<String> = contents
                        .split_inclusive('\n')
                        .map(|line| line.to_owned())
                        .collect();
                    Ok(BlobContents::Text(lines))
                }
            }
        };

        // FIXME: should we rely on the caller to add the file contents to
        // the ODB?
        match repo.inner.blob_path(&path) {
            Ok(_) => {}
            Err(err) if err.code() == git2::ErrorCode::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        let before_lines = get_lines_from_blob(old_oid)?;
        let after_lines = get_lines_from_blob(new_oid)?;

        let mut unchanged_hunk_line_idx = 0;
        let mut file_sections = Vec::new();
        for hunk in hunks {
            #[derive(Debug)]
            enum Lines<'a> {
                Lines(&'a [String]),
                BinaryDescription(String),
            }
            let empty_lines: Vec<String> = Default::default();
            let before_lines = match &before_lines {
                BlobContents::Absent => Lines::Lines(&empty_lines),
                BlobContents::Text(before_lines) => Lines::Lines(before_lines),
                BlobContents::Binary(num_bytes) => Lines::BinaryDescription(
                    make_binary_description(&old_oid.to_string(), *num_bytes),
                ),
            };
            let after_lines = match &after_lines {
                BlobContents::Absent => Lines::Lines(Default::default()),
                BlobContents::Text(after_lines) => Lines::Lines(after_lines),
                BlobContents::Binary(num_bytes) => Lines::BinaryDescription(
                    make_binary_description(&new_oid.to_string(), *num_bytes),
                ),
            };

            let (before_lines, after_lines) = match (before_lines, after_lines) {
                (Lines::Lines(before_lines), Lines::Lines(after_lines)) => {
                    (before_lines, after_lines)
                }
                (Lines::BinaryDescription(_), Lines::Lines(after_lines)) => {
                    (Default::default(), after_lines)
                }
                (Lines::Lines(_), Lines::BinaryDescription(new_description)) => {
                    file_sections.push(Section::Binary {
                        is_checked: false,
                        old_description: None,
                        new_description: Some(Cow::Owned(new_description)),
                    });
                    continue;
                }
                (
                    Lines::BinaryDescription(old_description),
                    Lines::BinaryDescription(new_description),
                ) => {
                    file_sections.push(Section::Binary {
                        is_checked: false,
                        old_description: Some(Cow::Owned(old_description)),
                        new_description: Some(Cow::Owned(new_description)),
                    });
                    continue;
                }
            };

            let GitHunk {
                old_start,
                old_lines,
                new_start,
                new_lines,
            } = hunk;

            // The line numbers are one-indexed.
            let (old_start, old_is_empty) = if old_start == 0 && old_lines == 0 {
                (0, true)
            } else {
                assert!(old_start > 0);
                (old_start - 1, false)
            };
            let new_start = if new_start == 0 && new_lines == 0 {
                0
            } else {
                assert!(new_start > 0);
                new_start - 1
            };

            // If we're starting a new hunk, first paste in any unchanged
            // lines since the last hunk (from the old version of the file).
            if unchanged_hunk_line_idx <= old_start {
                let end = if old_lines == 0 && !old_is_empty {
                    // Insertions are indicated with `old_lines == 0`, but in
                    // those cases, the inserted line is *after* the provided
                    // line number.
                    old_start + 1
                } else {
                    old_start
                };
                file_sections.push(Section::Unchanged {
                    lines: before_lines[unchanged_hunk_line_idx..end]
                        .iter()
                        .cloned()
                        .map(Cow::Owned)
                        .collect_vec(),
                });
                unchanged_hunk_line_idx = end + old_lines;
            }

            let before_idx_start = old_start;
            let before_idx_end = before_idx_start + old_lines;
            assert!(
                before_idx_end <= before_lines.len(),
                "before_idx_end {end} was not in range [0, {len}): {hunk:?}, path: {path:?}; lines {start}-... are: {lines:?}",
                start = before_idx_start,
                end = before_idx_end,
                len = before_lines.len(),
                hunk = hunk,
                path = path,
                lines = &before_lines[before_idx_start..],
            );
            let before_section_lines = before_lines[before_idx_start..before_idx_end]
                .iter()
                .cloned()
                .map(|before_line| SectionChangedLine {
                    is_checked: false,
                    change_type: ChangeType::Removed,
                    line: Cow::Owned(before_line),
                })
                .collect_vec();

            let after_idx_start = new_start;
            let after_idx_end = after_idx_start + new_lines;
            assert!(
                after_idx_end <= after_lines.len(),
                "after_idx_end {end} was not in range [0, {len}): {hunk:?}, path: {path:?}; lines {start}-... are: {lines:?}",
                start = after_idx_start,
                end = after_idx_end,
                len = after_lines.len(),
                hunk = hunk,
                path = path,
                lines = &after_lines[after_idx_start..],
            );
            let after_section_lines = after_lines[after_idx_start..after_idx_end]
                .iter()
                .cloned()
                .map(|after_line| SectionChangedLine {
                    is_checked: false,
                    change_type: ChangeType::Added,
                    line: Cow::Owned(after_line),
                })
                .collect_vec();

            if !(before_section_lines.is_empty() && after_section_lines.is_empty()) {
                file_sections.push(Section::Changed {
                    lines: before_section_lines
                        .into_iter()
                        .chain(after_section_lines)
                        .collect(),
                });
            }
        }

        if let BlobContents::Text(before_lines) = before_lines {
            if unchanged_hunk_line_idx < before_lines.len() {
                file_sections.push(Section::Unchanged {
                    lines: before_lines[unchanged_hunk_line_idx..]
                        .iter()
                        .cloned()
                        .map(Cow::Owned)
                        .collect(),
                });
            }
        }

        let file_mode_section = if old_file_mode != new_file_mode {
            vec![Section::FileMode {
                is_checked: false,
                mode: new_file_mode,
            }]
        } else {
            vec![]
        };
        result.push(File {
            old_path: None,
            path: Cow::Owned(path),
            file_mode: old_file_mode,
            sections: [file_mode_section, file_sections].concat().to_vec(),
        });
    }

    result.sort_by_cached_key(|file| file.path.clone().into_owned());
    Ok(result)
}
