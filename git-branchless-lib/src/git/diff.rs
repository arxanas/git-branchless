use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eyre::Context;
use git_record::{FileContent, Hunk, HunkChangedLine};

use super::{MaybeZeroOid, Repo};

/// A diff between two trees/commits.
pub struct Diff<'repo> {
    pub(super) inner: git2::Diff<'repo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct GitHunk {
    old_start: usize,
    old_lines: usize,
    new_start: usize,
    new_lines: usize,
}

/// Calculate the diff between the index and the working copy.
pub fn process_diff_for_record(
    repo: &Repo,
    diff: &Diff,
) -> eyre::Result<Vec<(PathBuf, FileContent)>> {
    let Diff { inner: diff } = diff;

    #[derive(Clone, Debug)]
    enum DeltaFileContent {
        Hunks(Vec<GitHunk>),
        Binary,
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
                content: DeltaFileContent::Binary,
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
                DeltaFileContent::Binary => {
                    panic!(
                        "File {:?} got a hunk callback, but it was a binary file",
                        path
                    )
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

        if new_oid.is_zero() {
            result.push((path, FileContent::Absent));
            continue;
        }

        let hunks = match content {
            DeltaFileContent::Binary => {
                result.push((path, FileContent::Binary));
                continue;
            }
            DeltaFileContent::Hunks(mut hunks) => {
                hunks.sort_by_key(|hunk| (hunk.old_start, hunk.old_lines));
                hunks
            }
        };
        let get_lines_from_blob = |oid| -> eyre::Result<Option<Vec<String>>> {
            let oid = MaybeZeroOid::from(oid);
            match oid {
                MaybeZeroOid::Zero => Ok(Default::default()),
                MaybeZeroOid::NonZero(oid) => {
                    let contents = repo.find_blob_or_fail(oid)?.get_content().to_vec();
                    let contents = match String::from_utf8(contents) {
                        Ok(contents) => contents,
                        Err(_) => {
                            return Ok(None);
                        }
                    };
                    let lines: Vec<String> = contents
                        .split_inclusive('\n')
                        .map(|line| line.to_owned())
                        .collect();
                    Ok(Some(lines))
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
        let before_lines = match get_lines_from_blob(old_oid)? {
            Some(lines) => lines,
            None => {
                result.push((path, FileContent::Binary));
                continue;
            }
        };
        let after_lines = match get_lines_from_blob(new_oid)? {
            Some(lines) => lines,
            None => {
                result.push((path, FileContent::Binary));
                continue;
            }
        };

        let mut unchanged_hunk_line_idx = 0;
        let mut file_hunks = Vec::new();
        for hunk in hunks {
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
                file_hunks.push(Hunk::Unchanged {
                    contents: before_lines[unchanged_hunk_line_idx..end].to_vec(),
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
                lines  = &after_lines[after_idx_start..],
            );
            file_hunks.push(Hunk::Changed {
                before: before_lines[before_idx_start..before_idx_end]
                    .iter()
                    .cloned()
                    .map(|line| HunkChangedLine {
                        is_selected: false,
                        line,
                    })
                    .collect(),
                after: after_lines[after_idx_start..after_idx_end]
                    .iter()
                    .cloned()
                    .map(|line| HunkChangedLine {
                        is_selected: false,
                        line,
                    })
                    .collect(),
            });
        }

        if unchanged_hunk_line_idx < before_lines.len() {
            file_hunks.push(Hunk::Unchanged {
                contents: before_lines[unchanged_hunk_line_idx..].to_vec(),
            });
        }
        result.push((
            path,
            FileContent::Text {
                file_mode: (
                    u32::from(old_file_mode).try_into().unwrap(),
                    u32::from(new_file_mode).try_into().unwrap(),
                ),
                hunks: file_hunks,
            },
        ));
    }
    Ok(result)
}
