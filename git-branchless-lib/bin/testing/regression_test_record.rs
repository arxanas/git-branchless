//! Test to make sure that `git record` would produce the same results as regular Git
//! when applying a patch.

use std::collections::HashMap;
use std::path::PathBuf;

use branchless::core::effects::Effects;
use branchless::core::formatting::Glyphs;
use branchless::git::{
    hydrate_tree, process_diff_for_record, Commit, FileMode, MaybeZeroOid, NonZeroOid, Repo, Tree,
};
use bstr::ByteSlice;
use eyre::Context;
use scm_record::{File, SelectedContents};

fn entries_from_files(
    repo: &Repo,
    old_tree: &Tree,
    new_tree: &Tree,
    files: &[File],
) -> eyre::Result<HashMap<PathBuf, Option<(NonZeroOid, FileMode)>>> {
    let entries = files
        .iter()
        .map(|file| {
            let file_path = file.path.clone().into_owned();
            let value = {
                let (selected, _unselected) = file.get_selected_contents();
                let blob_oid = match selected {
                    SelectedContents::Absent => return Ok((file_path, None)),
                    SelectedContents::Unchanged => {
                        old_tree.get_oid_for_path(&file.path)?.unwrap_or_default()
                    }
                    SelectedContents::Binary {
                        old_description: _,
                        new_description: _,
                    } => new_tree.get_oid_for_path(&file.path)?.unwrap(),
                    SelectedContents::Present { contents } => {
                        MaybeZeroOid::NonZero(repo.create_blob_from_contents(contents.as_bytes())?)
                    }
                };
                match blob_oid {
                    MaybeZeroOid::Zero => None,
                    MaybeZeroOid::NonZero(blob_oid) => {
                        let new_file_mode = file
                            .get_file_mode()
                            .expect("File mode should have been set");
                        let file_mode = i32::try_from(new_file_mode).unwrap();
                        let file_mode = FileMode::from(file_mode);
                        Some((blob_oid, file_mode))
                    }
                }
            };
            Ok((file_path, value))
        })
        .collect::<eyre::Result<_>>()?;
    Ok(entries)
}

fn assert_trees_equal(
    test: &str,
    repo: &Repo,
    parent_commit: &Commit,
    current_commit: &Commit,
    expected_tree: &Tree,
    entries: &[File],
) -> eyre::Result<()> {
    let old_tree = parent_commit.get_tree()?;
    let new_tree = current_commit.get_tree()?;
    let entries = entries_from_files(repo, &old_tree, &new_tree, entries)?;
    let actual_tree_oid = hydrate_tree(repo, Some(&old_tree), entries)?;
    let actual_tree = repo.find_tree_or_fail(actual_tree_oid)?;
    let actual_commit = {
        let author = current_commit.get_author();
        let committer = current_commit.get_committer();
        let message = current_commit.get_message_raw();
        let message = message.to_str_lossy();
        let parents = current_commit.get_parents();
        let actual_oid = repo.create_commit(
            &author,
            &committer,
            &message,
            &actual_tree,
            parents.iter().collect(),
            None,
        )?;
        repo.find_commit_or_fail(actual_oid)?
    };
    if actual_tree.get_oid() != expected_tree.get_oid() {
        eyre::bail!(
            "\
Trees are NOT equal for test {test:?}
Actual: {actual} vs expected: {expected}
Try running:
git diff-tree -p {expected} {actual}
Or examine the new (wrong) commit with:
git show {commit_oid}",
            expected = expected_tree.get_oid().to_string(),
            actual = actual_tree.get_oid().to_string(),
            commit_oid = actual_commit.get_oid(),
        );
    }

    Ok(())
}

fn main() -> eyre::Result<()> {
    let path_to_repo = std::env::var("PATH_TO_REPO")
        .wrap_err("Could not read PATH_TO_REPO environment variable")?;
    let repo = Repo::from_dir(&PathBuf::from(path_to_repo))?;
    let glyphs = Glyphs::detect();
    let effects = Effects::new(glyphs);

    let mut parent_commit = repo.find_commit_or_fail(repo.get_head_info()?.oid.unwrap())?;
    for i in 1..1000 {
        let current_commit = parent_commit;
        parent_commit = match current_commit.get_parents().first() {
            Some(parent_commit) => parent_commit.clone(),
            None => {
                println!("Reached root commit, exiting.");
                break;
            }
        };
        println!("Test #{i}: {current_commit:?}");

        let old_tree = parent_commit.get_tree()?;
        let new_tree = current_commit.get_tree()?;
        let diff = repo.get_diff_between_trees(&effects, Some(&old_tree), &new_tree, 0)?;

        let files = process_diff_for_record(&repo, &diff)?;
        {
            assert_trees_equal(
                &format!("select-none {parent_commit:?}"),
                &repo,
                &parent_commit,
                &current_commit,
                &parent_commit.get_tree()?,
                &files,
            )?;
        }

        // Select all changes (the resulting tree should be identical).
        {
            let mut files = files;
            for file in &mut files {
                file.set_checked(true);
            }
            assert_trees_equal(
                &format!("select-all {current_commit:?}"),
                &repo,
                &parent_commit,
                &current_commit,
                &current_commit.get_tree()?,
                &files,
            )?;
        }
    }

    Ok(())
}
