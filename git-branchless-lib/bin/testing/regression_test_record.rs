//! Test to make sure that `git record` would produce the same results as regular Git
//! when applying a patch.

use std::collections::HashMap;
use std::path::PathBuf;

use branchless::core::effects::Effects;
use branchless::core::formatting::Glyphs;
use branchless::git::{hydrate_tree, process_diff_for_record, FileMode, Repo};
use eyre::Context;
use git_record::{FileContent, Hunk};

fn main() -> eyre::Result<()> {
    let path_to_repo = std::env::var("PATH_TO_REPO")
        .wrap_err("Could not read PATH_TO_REPO environment variable")?;
    let repo = Repo::from_dir(&PathBuf::from(path_to_repo))?;
    let glyphs = Glyphs::detect();
    let effects = Effects::new(glyphs);

    let mut next_commit = repo.find_commit_or_fail(repo.get_head_info()?.oid.unwrap())?;
    for i in 1..1000 {
        let current_commit = next_commit;
        next_commit = match current_commit.get_parents().first() {
            Some(parent_commit) => parent_commit.clone(),
            None => {
                println!("Reached root commit, exiting.");
                break;
            }
        };
        println!("Test #{}: {:?}", i, current_commit);

        let parent_commit = match current_commit.get_only_parent() {
            Some(parent_commit) => parent_commit,
            None => {
                println!(
                    "Skipping since commit had multiple parents: {:?}",
                    current_commit.get_parents(),
                );
                continue;
            }
        };

        let old_tree = parent_commit.get_tree()?;
        let new_tree = current_commit.get_tree()?;
        let diff = repo.get_diff_between_trees(&effects, Some(&old_tree), &new_tree, 0)?;

        let entries = {
            let mut entries = process_diff_for_record(&repo, &diff)?;
            for (_, file_content) in &mut entries {
                match file_content {
                    FileContent::Absent => {}
                    FileContent::Text {
                        file_mode: _,
                        hunks,
                    } => {
                        for hunk in hunks {
                            match hunk {
                                Hunk::Unchanged { contents: _ } => {}
                                Hunk::Changed { before, after } => {
                                    for changed_line in before {
                                        changed_line.is_selected = true;
                                    }
                                    for changed_line in after {
                                        changed_line.is_selected = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            entries
        };
        let entries: HashMap<_, _> = entries
            .into_iter()
            .map(|(path, file_content)| {
                let value = match file_content {
                    FileContent::Absent => None,
                    FileContent::Text {
                        file_mode: (_old_file_mode, new_file_mode),
                        hunks: _,
                    } => {
                        let (selected, _unselected) = file_content.get_selected_contents();
                        let blob_oid = repo.create_blob_from_contents(selected.as_bytes())?;
                        let file_mode = i32::try_from(new_file_mode).unwrap();
                        let file_mode = FileMode::from(file_mode);
                        Some((blob_oid, file_mode))
                    }
                };
                Ok((path, value))
            })
            .collect::<eyre::Result<_>>()?;

        let actual_tree_oid = hydrate_tree(&repo, Some(&old_tree), entries)?;
        let actual_tree = repo.find_tree_or_fail(actual_tree_oid)?;
        let actual_commit = {
            let author = current_commit.get_author();
            let committer = current_commit.get_committer();
            let message = current_commit.get_message_raw()?;
            let message = message.to_string_lossy();
            let parents = current_commit.get_parents();
            let actual_oid = repo.create_commit(
                None,
                &author,
                &committer,
                &message,
                &actual_tree,
                parents.iter().collect(),
            )?;
            repo.find_commit_or_fail(actual_oid)?
        };
        let expected_tree = current_commit.get_tree()?;
        if actual_tree.get_oid() != expected_tree.get_oid() {
            println!(
                "Trees are NOT equal, actual {actual} vs expected {expected}\n\
                Try running:\n\
                git diff-tree -p {expected} {actual}\n\
                Or examine the new (wrong) commit with:\n\
                git show {commit_oid}",
                expected = expected_tree.get_oid().to_string(),
                actual = actual_tree.get_oid().to_string(),
                commit_oid = actual_commit.get_oid(),
            );
            std::process::exit(1);
        }
    }

    Ok(())
}
