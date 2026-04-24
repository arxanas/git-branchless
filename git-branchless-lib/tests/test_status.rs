use std::fs;
use std::path::PathBuf;

use branchless::core::effects::Effects;
use branchless::core::formatting::Glyphs;
use branchless::git::{FileMode, FileStatus, StatusEntry, WorkingCopyChangesType};
use branchless::testing::{GitInitOptions, GitRunOptions, make_git};

#[test]
fn test_parse_status_line() {
    assert_eq!(
            StatusEntry::try_from(
                "1 .M N... 100644 100644 100644 51fcbe2362663a19d132767b69c2c7829023f3da 51fcbe2362663a19d132767b69c2c7829023f3da repo.rs".as_bytes(),
            ).unwrap(),
            StatusEntry {
                index_status: FileStatus::Unmodified,
                working_copy_status: FileStatus::Modified,
                path: "repo.rs".into(),
                orig_path: None,
                working_copy_file_mode: FileMode::Blob,
            }
        );

    assert_eq!(
            StatusEntry::try_from(
                "1 .M N... 100644 100644 100644 51fcbe2362663a19d132767b69c2c7829023f3da 51fcbe2362663a19d132767b69c2c7829023f3da filename with spaces.rs".as_bytes(),
            ).unwrap(),
            StatusEntry {
                index_status: FileStatus::Unmodified,
                working_copy_status: FileStatus::Modified,
                path: "filename with spaces.rs".into(),
                orig_path: None,
                working_copy_file_mode: FileMode::Blob,
            }
        );

    assert_eq!(
            StatusEntry::try_from(
                "1 A. N... 100755 100755 100755 51fcbe2362663a19d132767b69c2c7829023f3da 51fcbe2362663a19d132767b69c2c7829023f3da repo.rs".as_bytes(),
            ).unwrap(),
            StatusEntry {
                index_status: FileStatus::Added,
                working_copy_status: FileStatus::Unmodified,
                path: "repo.rs".into(),
                orig_path: None,
                working_copy_file_mode: FileMode::BlobExecutable,
            }
        );

    assert_eq!(
            StatusEntry::try_from(
                "1 A. N... 100755 100755 100755 51fcbe2362663a19d132767b69c2c7829023f3da 51fcbe2362663a19d132767b69c2c7829023f3da filename with spaces.rs".as_bytes(),
            ).unwrap(),
            StatusEntry {
                index_status: FileStatus::Added,
                working_copy_status: FileStatus::Unmodified,
                path: "filename with spaces.rs".into(),
                orig_path: None,
                working_copy_file_mode: FileMode::BlobExecutable,
            }
        );

    let entry: StatusEntry = StatusEntry::try_from(
            "2 RD N... 100644 100644 100644 9daeafb9864cf43055ae93beb0afd6c7d144bfa4 9daeafb9864cf43055ae93beb0afd6c7d144bfa4 R100 new_file.rs\x00old_file.rs".as_bytes(),
        ).unwrap();
    assert_eq!(
        entry,
        StatusEntry {
            index_status: FileStatus::Renamed,
            working_copy_status: FileStatus::Deleted,
            path: "new_file.rs".into(),
            orig_path: Some("old_file.rs".into()),
            working_copy_file_mode: FileMode::Blob,
        }
    );
    assert_eq!(
        entry.paths(),
        vec![PathBuf::from("new_file.rs"), PathBuf::from("old_file.rs")]
    );

    assert_eq!(
            StatusEntry::try_from(
                "u A. N... 100755 100755 100755 100755 51fcbe2362663a19d132767b69c2c7829023f3da 51fcbe2362663a19d132767b69c2c7829023f3da 9daeafb9864cf43055ae93beb0afd6c7d144bfa4 repo.rs".as_bytes(),
            ).unwrap(),
            StatusEntry {
                index_status: FileStatus::Unmerged,
                working_copy_status: FileStatus::Unmodified,
                path: "repo.rs".into(),
                orig_path: None,
                working_copy_file_mode: FileMode::BlobExecutable,
            }
        );

    assert_eq!(
            StatusEntry::try_from(
                "u A. N... 100755 100755 100755 100755 51fcbe2362663a19d132767b69c2c7829023f3da 51fcbe2362663a19d132767b69c2c7829023f3da 9daeafb9864cf43055ae93beb0afd6c7d144bfa4 filename with spaces.rs".as_bytes(),
            ).unwrap(),
            StatusEntry {
                index_status: FileStatus::Unmerged,
                working_copy_status: FileStatus::Unmodified,
                path: "filename with spaces.rs".into(),
                orig_path: None,
                working_copy_file_mode: FileMode::BlobExecutable,
            }
        );
}

#[test]
fn test_get_status() -> eyre::Result<()> {
    let git = make_git()?;
    let git_run_info = git.get_git_run_info();
    git.init_repo()?;
    git.commit_file("test1", 1)?;

    let glyphs = Glyphs::text();
    let effects = Effects::new_suppress_for_test(glyphs);
    let repo = git.get_repo()?;

    let (snapshot, status) = repo.get_status(
        &effects,
        &git_run_info,
        &repo.get_index()?,
        &repo.get_head_info()?,
        None,
    )?;
    assert_eq!(
        snapshot.get_working_copy_changes_type()?,
        WorkingCopyChangesType::None
    );
    assert_eq!(status, vec![]);
    insta::assert_debug_snapshot!(snapshot, @r###"
        WorkingCopySnapshot {
            base_commit: Commit {
                inner: Commit {
                    id: ad8334119626cc9aee5322f9ed35273de834ea36,
                    summary: "branchless: automated working copy snapshot",
                },
            },
            head_commit: Some(
                Commit {
                    inner: Commit {
                        id: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                        summary: "create test1.txt",
                    },
                },
            ),
            head_reference_name: Some(
                ReferenceName(
                    "refs/heads/master",
                ),
            ),
            commit_unstaged: Commit {
                inner: Commit {
                    id: cd8605eef8b78e22427fa3846f1a23f95e88aa7e,
                    summary: "branchless: working copy snapshot data: 0 unstaged changes",
                },
            },
            commit_stage0: Commit {
                inner: Commit {
                    id: a4edb48b44f5b19d0c2c25fd65251d0bfaba68c1,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 0",
                },
            },
            commit_stage1: Commit {
                inner: Commit {
                    id: e1e0c856237e53e2c889a723ee7ec50a21c1f952,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 1",
                },
            },
            commit_stage2: Commit {
                inner: Commit {
                    id: e5dda473c3266aafa14b827b1a009e35a3c61679,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 2",
                },
            },
            commit_stage3: Commit {
                inner: Commit {
                    id: 19b98ca24cc7b241122593fc1a9307e24e26a846,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 3",
                },
            },
        }
        "###);

    git.write_file_txt("new_file", "another file")?;
    git.run(&["add", "new_file.txt"])?;
    git.write_file_txt("untracked", "should not show up in status")?;
    git.delete_file("initial")?;
    git.run(&["mv", "test1.txt", "renamed.txt"])?;

    let (snapshot, status) = repo.get_status(
        &effects,
        &git_run_info,
        &repo.get_index()?,
        &repo.get_head_info()?,
        None,
    )?;
    assert_eq!(
        snapshot.get_working_copy_changes_type()?,
        WorkingCopyChangesType::Staged
    );
    assert_eq!(
        status,
        vec![
            StatusEntry {
                index_status: FileStatus::Unmodified,
                working_copy_status: FileStatus::Deleted,
                working_copy_file_mode: FileMode::Unreadable,
                path: "initial.txt".into(),
                orig_path: None
            },
            StatusEntry {
                index_status: FileStatus::Added,
                working_copy_status: FileStatus::Unmodified,
                working_copy_file_mode: FileMode::Blob,
                path: "new_file.txt".into(),
                orig_path: None
            },
            StatusEntry {
                index_status: FileStatus::Renamed,
                working_copy_status: FileStatus::Unmodified,
                working_copy_file_mode: FileMode::Blob,
                path: "renamed.txt".into(),
                orig_path: Some("test1.txt".into())
            }
        ]
    );
    insta::assert_debug_snapshot!(snapshot, @r###"
        WorkingCopySnapshot {
            base_commit: Commit {
                inner: Commit {
                    id: 2f8be3a55bd58854a5871d7260abc37ab5d1199c,
                    summary: "branchless: automated working copy snapshot",
                },
            },
            head_commit: Some(
                Commit {
                    inner: Commit {
                        id: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                        summary: "create test1.txt",
                    },
                },
            ),
            head_reference_name: Some(
                ReferenceName(
                    "refs/heads/master",
                ),
            ),
            commit_unstaged: Commit {
                inner: Commit {
                    id: 329d438eaf36089efa9a49a3942a10985c037dce,
                    summary: "branchless: working copy snapshot data: 4 unstaged changes",
                },
            },
            commit_stage0: Commit {
                inner: Commit {
                    id: ccfd588cf59116f67664ac718c404b09fc9e35d2,
                    summary: "branchless: working copy snapshot data: 3 changes in stage 0",
                },
            },
            commit_stage1: Commit {
                inner: Commit {
                    id: e1e0c856237e53e2c889a723ee7ec50a21c1f952,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 1",
                },
            },
            commit_stage2: Commit {
                inner: Commit {
                    id: e5dda473c3266aafa14b827b1a009e35a3c61679,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 2",
                },
            },
            commit_stage3: Commit {
                inner: Commit {
                    id: 19b98ca24cc7b241122593fc1a9307e24e26a846,
                    summary: "branchless: working copy snapshot data: 0 changes in stage 3",
                },
            },
        }
        "###);

    Ok(())
}

#[test]
fn test_get_status_with_dirty_submodule() -> eyre::Result<()> {
    let git = make_git()?;
    let git_run_info = git.get_git_run_info();
    git.init_repo_with_options(&GitInitOptions {
        make_initial_commit: true,
        run_branchless_init: false,
    })?;
    let glyphs = Glyphs::text();
    let effects = Effects::new_suppress_for_test(glyphs);
    let repo = git.get_repo()?;

    let submodule_source_path = git.repo_path.join("submodule-source");
    let submodule_source_path_str = submodule_source_path
        .to_str()
        .expect("submodule source path should be UTF-8");

    {
        // create repo for submodule
        fs::create_dir(&submodule_source_path)?;
        git.run(&["-C", submodule_source_path_str, "init"])?;
        git.run(&[
            "-C",
            submodule_source_path_str,
            "config",
            "user.name",
            "Testy McTestface",
        ])?;
        git.run(&[
            "-C",
            submodule_source_path_str,
            "config",
            "user.email",
            "test@example.com",
        ])?;
        fs::write(submodule_source_path.join("file.txt"), "contents\n")?;
        git.run(&["-C", submodule_source_path_str, "add", "file.txt"])?;
        git.run(&["-C", submodule_source_path_str, "commit", "-m", "initial"])?;
    }

    {
        // add submodule to main repo
        git.run_with_options(
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                submodule_source_path_str,
                "sm",
            ],
            &GitRunOptions::default(),
        )?;
        git.run(&["add", ".gitmodules", "sm"])?;
        git.run(&["commit", "-m", "add submodule"])?;
    }

    {
        // make change in submodule
        git.run(&["-C", "sm", "config", "user.name", "Testy McTestface"])?;
        git.run(&["-C", "sm", "config", "user.email", "test@example.com"])?;
        fs::write(git.repo_path.join("sm/file.txt"), "updated\n")?;
        git.run(&["-C", "sm", "commit", "-am", "update"])?;
    }

    let (_snapshot, status) = repo.get_status(
        &effects,
        &git_run_info,
        &repo.get_index()?,
        &repo.get_head_info()?,
        None,
    )?;

    // Should contain a single entry for the modified submodule.
    insta::assert_debug_snapshot!(status, @r###"
        [
            StatusEntry {
                index_status: Unmodified,
                working_copy_status: Modified,
                working_copy_file_mode: Commit,
                path: "sm",
                orig_path: None,
            },
        ]
    "###);

    Ok(())
}

#[cfg(unix)]
#[test]
fn test_get_status_with_directory_symlink_trailing_slash() -> eyre::Result<()> {
    let git = make_git()?;
    let git_run_info = git.get_git_run_info();
    git.init_repo_with_options(&GitInitOptions {
        make_initial_commit: false,
        run_branchless_init: false,
    })?;
    let glyphs = Glyphs::text();
    let effects = Effects::new_suppress_for_test(glyphs);
    let repo = git.get_repo()?;

    {
        fs::create_dir(git.repo_path.join("dir"))?;
        fs::write(git.repo_path.join("dir/file.txt"), "contents\n")?;
        std::os::unix::fs::symlink("dir/", git.repo_path.join("dir_symlink"))?;
        git.run(&["add", "dir/file.txt", "dir_symlink"])?;
    }

    let (_snapshot, status) = repo.get_status(
        &effects,
        &git_run_info,
        &repo.get_index()?,
        &repo.get_head_info()?,
        None,
    )?;

    // Should contain new (added) entries for 2 files: dir/file.txt (blob) and
    // dir_symlink (link)
    insta::assert_debug_snapshot!(status, @r###"
        [
            StatusEntry {
                index_status: Added,
                working_copy_status: Unmodified,
                working_copy_file_mode: Blob,
                path: "dir/file.txt",
                orig_path: None,
            },
            StatusEntry {
                index_status: Added,
                working_copy_status: Unmodified,
                working_copy_file_mode: Link,
                path: "dir_symlink",
                orig_path: None,
            },
        ]
    "###);

    Ok(())
}
