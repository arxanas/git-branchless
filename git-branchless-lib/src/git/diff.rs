use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eyre::Context;
use git_record::{FileContent, Hunk, HunkChangedLine};

use super::{MaybeZeroOid, NonZeroOid, Repo};

/// A diff between two trees/commits.
pub struct Diff<'repo> {
    pub(super) inner: git2::Diff<'repo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GitHunk {
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

    type Value = (git2::Oid, git2::Oid, Vec<GitHunk>);
    let hunks: Arc<Mutex<HashMap<PathBuf, Value>>> = Default::default();
    diff.foreach(
        &mut |delta, _| {
            let mut hunks = hunks.lock().unwrap();
            hunks.insert(
                delta.new_file().path().unwrap().into(),
                (
                    delta.old_file().id(),
                    delta.new_file().id(),
                    Default::default(),
                ),
            );
            true
        },
        Some(&mut |delta, _| {
            todo!(
                "Binary diffing not implemented (for file: {:?})",
                delta.new_file()
            )
        }),
        Some(&mut |delta, hunk| {
            let path = delta.new_file().path().unwrap();
            let mut hunks = hunks.lock().unwrap();
            hunks.get_mut(path).unwrap().2.push(GitHunk {
                old_start: hunk.old_start().try_into().unwrap(),
                old_lines: hunk.old_lines().try_into().unwrap(),
                new_start: hunk.new_start().try_into().unwrap(),
                new_lines: hunk.new_lines().try_into().unwrap(),
            });
            true
        }),
        None,
    )
    .wrap_err("Iterating over diff")?;

    let hunks = std::mem::take(&mut *hunks.lock().unwrap());
    let mut result = Vec::new();
    for (path, (old_oid, new_oid, hunks)) in hunks {
        let get_lines_from_blob = |oid| -> eyre::Result<Vec<String>> {
            let oid = MaybeZeroOid::from(oid);
            let oid = NonZeroOid::try_from(oid)?;
            let contents = repo.find_blob_or_fail(oid)?.get_content().to_vec();
            let contents = String::from_utf8(contents).wrap_err("Decoding old file contents")?;
            let lines: Vec<String> = contents.lines().map(|line| line.to_owned()).collect();
            Ok(lines)
        };
        repo.inner.blob_path(&path)?;
        let before_lines = get_lines_from_blob(old_oid)?;
        let after_lines = get_lines_from_blob(new_oid)?;

        let mut before_line_idx = 0;
        let mut file_hunks = Vec::new();
        for hunk in hunks {
            let GitHunk {
                old_start,
                old_lines,
                new_start,
                new_lines,
            } = hunk;
            if before_line_idx < old_start {
                file_hunks.push(Hunk::Unchanged {
                    contents: before_lines[before_line_idx..old_start].to_vec(),
                });
                before_line_idx = old_start + old_lines - 1;
            }
            file_hunks.push(Hunk::Changed {
                before: before_lines[old_start..old_start + old_lines - 1]
                    .iter()
                    .cloned()
                    .map(|line| HunkChangedLine {
                        is_selected: false,
                        line,
                    })
                    .collect(),
                after: after_lines[new_start..new_start + new_lines - 1]
                    .iter()
                    .cloned()
                    .map(|line| HunkChangedLine {
                        is_selected: false,
                        line,
                    })
                    .collect(),
            })
        }
        if before_line_idx < before_lines.len() {
            file_hunks.push(Hunk::Unchanged {
                contents: before_lines[before_line_idx..].to_vec(),
            });
        }
        result.push((path, FileContent::Text { hunks: file_hunks }));
    }
    Ok(result)
}
