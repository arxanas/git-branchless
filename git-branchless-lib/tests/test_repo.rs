use std::path::PathBuf;

use branchless::git::{
    AmendFastOptions, BranchType, CherryPickFastOptions, FileMode, FileStatus, GitVersion, Repo,
    StatusEntry,
};
use branchless::testing::{
    GitWorktreeWrapper, GitWrapperWithRemoteRepo, make_git, make_git_with_remote_repo,
    make_git_worktree,
};

#[test]
fn test_parse_git_version_output() {
    assert_eq!(
        "git version 12.34.56".parse::<GitVersion>().unwrap(),
        GitVersion(12, 34, 56)
    );
    assert_eq!(
        "git version 12.34.56\n".parse::<GitVersion>().unwrap(),
        GitVersion(12, 34, 56)
    );
    assert_eq!(
        "git version 12.34.56.78.abcdef"
            .parse::<GitVersion>()
            .unwrap(),
        GitVersion(12, 34, 56)
    );

    // See https://github.com/arxanas/git-branchless/issues/69
    assert_eq!(
        "git version 2.33.0-rc0".parse::<GitVersion>().unwrap(),
        GitVersion(2, 33, 0)
    );

    // See https://github.com/arxanas/git-branchless/issues/85
    assert_eq!(
        "git version 2.33.GIT".parse::<GitVersion>().unwrap(),
        GitVersion(2, 33, 0)
    );
}

#[test]
fn test_cherry_pick_fast() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.run(&["checkout", "-b", "foo"])?;
    let test1_oid = git.commit_file_with_contents("test1", 1, "test1 contents")?;
    git.run(&["checkout", "master"])?;
    let initial2_oid = git.commit_file_with_contents("initial", 2, "updated initial contents")?;

    let repo = git.get_repo()?;
    let test1_commit = repo.find_commit_or_fail(test1_oid)?;
    let initial2_commit = repo.find_commit_or_fail(initial2_oid)?;
    let tree = repo.cherry_pick_fast(
        &test1_commit,
        &initial2_commit,
        &CherryPickFastOptions {
            reuse_parent_tree_if_possible: false,
        },
    )?;

    insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 367f91ddd5df2d1c18742ce3f09b4944944cac3a,
            },
        }
        "###);

    insta::assert_debug_snapshot!(tree.get_entry_paths_for_testing(), @r###"
        [
            "initial.txt",
            "test1.txt",
        ]
        "###);

    Ok(())
}

#[test]
fn test_amend_fast_from_index() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.run(&["checkout", "master"])?;
    let initial_oid = git.commit_file_with_contents("initial", 2, "initial contents")?;
    git.write_file_txt("initial", "updated contents")?;

    let repo = git.get_repo()?;
    let initial_commit = repo.find_commit_or_fail(initial_oid)?;

    let tree = initial_commit.get_tree()?;
    insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 01deb7745d411223bbf6b9cb1abaeed451bb25a0,
            },
        }
        "###);
    insta::assert_debug_snapshot!(tree.get_entries_for_testing(), @r###"
        [
            (
                "initial.txt",
                "5c41c3d7e736911dbbd53d62c10292b9bc78f838",
            ),
        ]
        "###);

    let tree = repo.amend_fast(
        &initial_commit,
        &AmendFastOptions::FromIndex {
            paths: vec!["initial.txt".into()],
        },
    )?;

    insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 01deb7745d411223bbf6b9cb1abaeed451bb25a0,
            },
        }
        "###);
    insta::assert_debug_snapshot!(tree.get_entries_for_testing(), @r###"
        [
            (
                "initial.txt",
                "5c41c3d7e736911dbbd53d62c10292b9bc78f838",
            ),
        ]
        "###);

    git.run(&["add", "initial.txt"])?;
    let tree = repo.amend_fast(
        &initial_commit,
        &AmendFastOptions::FromIndex {
            paths: vec!["initial.txt".into()],
        },
    )?;

    insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 1c15b79a72c3285df172fcfdaceedb7259283eb5,
            },
        }
        "###);
    insta::assert_debug_snapshot!(tree.get_entries_for_testing(), @r###"
        [
            (
                "initial.txt",
                "53cd9398c8a2d92f18d279c6cad3f5dde67235e7",
            ),
        ]
        "###);

    Ok(())
}

#[test]
fn test_amend_fast_from_working_tree() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.run(&["checkout", "master"])?;
    let initial_oid = git.commit_file_with_contents("initial", 2, "initial contents")?;
    git.write_file_txt("initial", "updated contents")?;

    let repo = git.get_repo()?;
    let initial_commit = repo.find_commit_or_fail(initial_oid)?;
    let tree = repo.amend_fast(
        &initial_commit,
        &AmendFastOptions::FromWorkingCopy {
            status_entries: vec![StatusEntry {
                index_status: FileStatus::Renamed,
                working_copy_status: FileStatus::Unmodified,
                working_copy_file_mode: FileMode::Blob,
                path: "initial.txt".into(),
                orig_path: None,
            }],
        },
    )?;

    insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 1c15b79a72c3285df172fcfdaceedb7259283eb5,
            },
        }
        "###);
    insta::assert_debug_snapshot!(tree.get_entries_for_testing(), @r###"
        [
            (
                "initial.txt",
                "53cd9398c8a2d92f18d279c6cad3f5dde67235e7",
            ),
        ]
        "###);

    git.write_file_txt("file2", "another file")?;
    git.write_file_txt("initial", "updated contents again")?;
    let tree = repo.amend_fast(
        &initial_commit,
        &AmendFastOptions::FromWorkingCopy {
            status_entries: vec![StatusEntry {
                index_status: FileStatus::Unmodified,
                working_copy_status: FileStatus::Added,
                working_copy_file_mode: FileMode::Blob,
                path: "file2.txt".into(),
                orig_path: None,
            }],
        },
    )?;
    insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 1a9fbbecd825881c3e79f0fb194a1c1e1104fe0f,
            },
        }
        "###);
    insta::assert_debug_snapshot!(tree.get_entries_for_testing(), @r###"
        [
            (
                "file2.txt",
                "cdcb28483da7783a8b505a074c50632a5481a69b",
            ),
            (
                "initial.txt",
                "5c41c3d7e736911dbbd53d62c10292b9bc78f838",
            ),
        ]
        "###);

    git.delete_file("initial")?;
    let tree = repo.amend_fast(
        &initial_commit,
        &AmendFastOptions::FromWorkingCopy {
            status_entries: vec![StatusEntry {
                index_status: FileStatus::Unmodified,
                working_copy_status: FileStatus::Deleted,
                working_copy_file_mode: FileMode::Blob,
                path: "initial.txt".into(),
                orig_path: None,
            }],
        },
    )?;
    insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 4b825dc642cb6eb9a060e54bf8d69288fbee4904,
            },
        }
        "###);
    insta::assert_debug_snapshot!(tree.get_entries_for_testing(), @"[]");

    Ok(())
}

#[test]
fn test_branch_debug() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    let repo = git.get_repo()?;
    let branch = repo.find_branch("master", BranchType::Local)?.unwrap();
    insta::assert_debug_snapshot!(branch, @r###"<Branch name="master">"###);

    Ok(())
}

#[test]
fn test_worktree_working_copy_path() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    let GitWorktreeWrapper { temp_dir, worktree } = make_git_worktree(&git, "new-worktree")?;
    {
        let stdout = worktree.smartlog()?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d (master) create test1.txt
        "###);
    }

    fn canonicalize(path: Option<PathBuf>) -> PathBuf {
        match path {
            None => PathBuf::from("<none>"),
            Some(path) => {
                // On macOS, it looks like the temporary directory has to be
                // canonicalized as it may be `/var` vs. `/private/var`.
                std::fs::canonicalize(path).unwrap_or_else(|err| format!("<error: {err}>").into())
            }
        }
    }
    let worktree_repo = worktree.get_repo()?;
    assert_eq!(
        canonicalize(worktree_repo.get_working_copy_path()),
        canonicalize(Some(temp_dir.path().join("new-worktree")))
    );
    let directly_opened_worktree_repo = Repo::from_dir(&temp_dir.path().join("new-worktree"))?;
    assert_eq!(
        canonicalize(directly_opened_worktree_repo.get_working_copy_path()),
        canonicalize(worktree_repo.get_working_copy_path())
    );

    Ok(())
}

#[test]
fn test_open_worktree_parent_repo_bare_clone() -> eyre::Result<()> {
    let GitWrapperWithRemoteRepo {
        temp_dir: _guard_original,
        original_repo,
        cloned_repo,
    } = make_git_with_remote_repo()?;

    // Create a bare clone of the original repo.
    {
        original_repo.init_repo()?;

        // `git.clone_repo_into` reinitializes the destination after cloning,
        // adding a `.git` directory and losing the bare state. Run `git clone
        // --bare` directly instead.
        original_repo.run(&[
            "clone",
            "--bare",
            original_repo.repo_path.to_str().unwrap(),
            cloned_repo.repo_path.to_str().unwrap(),
        ])?;
    }

    // Create a linked worktree from the bare clone.
    let GitWorktreeWrapper {
        temp_dir: _guard_worktree,
        worktree,
    } = make_git_worktree(&cloned_repo, "new-worktree")?;

    let parent_repo = worktree
        .get_repo()?
        .open_worktree_parent_repo()?
        .expect("expected to find parent repo for worktree of bare clone");

    assert_eq!(
        std::fs::canonicalize(parent_repo.get_path())?,
        std::fs::canonicalize(cloned_repo.repo_path)?,
    );

    Ok(())
}
