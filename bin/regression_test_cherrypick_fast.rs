//! Test to make sure that `cherrypick_fast` produces the same results as
//! regular Git when applying a patch.

use std::path::PathBuf;

use branchless::git::{CherryPickFastOptions, Repo};
use eyre::Context;

fn main() -> eyre::Result<()> {
    let path_to_repo = std::env::var("PATH_TO_REPO")
        .wrap_err_with(|| "Could not read PATH_TO_REPO environment variable")?;
    let repo = Repo::from_dir(&PathBuf::from(path_to_repo))?;

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

        let tree = repo.cherrypick_fast(
            &current_commit,
            &parent_commit,
            &CherryPickFastOptions {
                reuse_parent_tree_if_possible: false,
            },
        )?;
        let tree = match tree {
            Ok(tree) => tree,
            Err(err) => {
                println!("Failed to cherry-pick: {:?}", err);
                std::process::exit(1);
            }
        };

        let expected_tree = current_commit.get_tree()?;
        if tree.get_oid() != expected_tree.get_oid() {
            println!(
                "Trees are NOT equal, actual {actual} vs expected {expected}\n\
                Try running: git diff-tree -p {expected} {actual}",
                expected = expected_tree.get_oid().to_string(),
                actual = tree.get_oid().to_string(),
            );
            std::process::exit(1);
        }
    }

    Ok(())
}
