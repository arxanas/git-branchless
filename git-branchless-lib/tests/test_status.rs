use std::path::PathBuf;

use branchless::core::effects::Effects;
use branchless::core::formatting::Glyphs;
use branchless::git::{FileMode, FileStatus, StatusEntry, WorkingCopyChangesType};
use branchless::testing::make_git;

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
