use std::collections::HashMap;
use std::path::{Path, PathBuf};

use branchless::git::{dehydrate_tree, get_changed_paths_between_trees, hydrate_tree, FileMode};
use branchless::testing::make_git;

#[test]
fn test_hydrate_tree() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    git.write_file_txt("foo", "foo")?;
    git.write_file_txt("bar/bar", "bar")?;
    git.write_file_txt("bar/baz", "qux")?;
    git.write_file_txt("xyzzy", "xyzzy")?;
    git.run(&["add", "."])?;
    git.run(&["commit", "-m", "commit"])?;

    let repo = git.get_repo()?;
    let head_oid = repo.get_head_info()?.oid.unwrap();
    let head_commit = repo.find_commit_or_fail(head_oid)?;
    let head_tree = head_commit.get_tree()?;

    insta::assert_debug_snapshot!(head_tree.get_entries_for_testing(), @r###"
    [
        (
            "bar",
            "778e23a1e80b1feb10e00b15b29a33315929c5b5",
        ),
        (
            "foo.txt",
            "19102815663d23f8b75a47e7a01965dcdc96468c",
        ),
        (
            "initial.txt",
            "63af22885f8665a312ba8b83db722134f1f8290d",
        ),
        (
            "xyzzy.txt",
            "7c465afc533f95ff7d2c91e18921f94aac8292fc",
        ),
    ]
    "###);

    {
        let hydrated_tree = {
            let hydrated_tree_oid = hydrate_tree(&repo, Some(&head_tree), {
                let mut result = HashMap::new();
                result.insert(
                    PathBuf::from("foo-copy.txt"),
                    Some((
                        head_tree
                            .get_oid_for_path(&PathBuf::from("foo.txt"))?
                            .unwrap()
                            .try_into()?,
                        FileMode::from(0o100644),
                    )),
                );
                result.insert(PathBuf::from("foo.txt"), None);
                result
            })?;
            repo.find_tree(hydrated_tree_oid)?.unwrap()
        };
        insta::assert_debug_snapshot!(hydrated_tree.get_entries_for_testing(), @r###"
        [
            (
                "bar",
                "778e23a1e80b1feb10e00b15b29a33315929c5b5",
            ),
            (
                "foo-copy.txt",
                "19102815663d23f8b75a47e7a01965dcdc96468c",
            ),
            (
                "initial.txt",
                "63af22885f8665a312ba8b83db722134f1f8290d",
            ),
            (
                "xyzzy.txt",
                "7c465afc533f95ff7d2c91e18921f94aac8292fc",
            ),
        ]
        "###);
    }

    {
        let hydrated_tree = {
            let hydrated_tree_oid = hydrate_tree(&repo, Some(&head_tree), {
                let mut result = HashMap::new();
                result.insert(PathBuf::from("bar/bar.txt"), None);
                result
            })?;
            repo.find_tree(hydrated_tree_oid)?.unwrap()
        };
        insta::assert_debug_snapshot!(hydrated_tree.get_entries_for_testing(), @r###"
        [
            (
                "bar",
                "08ee88e1c53fbd01ab76f136a4f2c9d759b981d0",
            ),
            (
                "foo.txt",
                "19102815663d23f8b75a47e7a01965dcdc96468c",
            ),
            (
                "initial.txt",
                "63af22885f8665a312ba8b83db722134f1f8290d",
            ),
            (
                "xyzzy.txt",
                "7c465afc533f95ff7d2c91e18921f94aac8292fc",
            ),
        ]
        "###);
    }

    {
        let hydrated_tree = {
            let hydrated_tree_oid = hydrate_tree(&repo, Some(&head_tree), {
                let mut result = HashMap::new();
                result.insert(PathBuf::from("bar/bar.txt"), None);
                result.insert(PathBuf::from("bar/baz.txt"), None);
                result
            })?;
            repo.find_tree(hydrated_tree_oid)?.unwrap()
        };
        insta::assert_debug_snapshot!(hydrated_tree.get_entries_for_testing(), @r###"
        [
            (
                "foo.txt",
                "19102815663d23f8b75a47e7a01965dcdc96468c",
            ),
            (
                "initial.txt",
                "63af22885f8665a312ba8b83db722134f1f8290d",
            ),
            (
                "xyzzy.txt",
                "7c465afc533f95ff7d2c91e18921f94aac8292fc",
            ),
        ]
        "###);
    }

    {
        let dehydrated_tree_oid = dehydrate_tree(
            &repo,
            &head_tree,
            &[Path::new("bar/baz.txt"), Path::new("foo.txt")],
        )?;
        let dehydrated_tree = repo.find_tree(dehydrated_tree_oid)?.unwrap();
        insta::assert_debug_snapshot!(dehydrated_tree.get_entries_for_testing(), @r###"
        [
            (
                "bar",
                "08ee88e1c53fbd01ab76f136a4f2c9d759b981d0",
            ),
            (
                "foo.txt",
                "19102815663d23f8b75a47e7a01965dcdc96468c",
            ),
        ]
        "###);
    }

    Ok(())
}

#[test]
fn test_detect_path_only_changed_file_mode() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.run(&["update-index", "--chmod=+x", "initial.txt"])?;
    git.run(&["commit", "-m", "update file mode"])?;

    let repo = git.get_repo()?;
    let oid = repo.get_head_info()?.oid.unwrap();
    let commit = repo.find_commit_or_fail(oid)?;

    let lhs = commit.get_only_parent().unwrap();
    let lhs_tree = lhs.get_tree()?;
    let rhs_tree = commit.get_tree()?;
    let changed_paths = get_changed_paths_between_trees(&repo, Some(&lhs_tree), Some(&rhs_tree))?;

    insta::assert_debug_snapshot!(changed_paths, @r###"
        {
            "initial.txt",
        }
        "###);

    Ok(())
}
