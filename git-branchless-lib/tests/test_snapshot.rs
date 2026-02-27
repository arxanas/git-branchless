use std::time::SystemTime;

use branchless::core::effects::Effects;
use branchless::core::eventlog::EventLogDb;
use branchless::core::formatting::Glyphs;
use branchless::git::WorkingCopyChangesType;
use branchless::testing::{GitRunOptions, make_git};

#[test]
fn test_has_conflicts() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file_with_contents("test2", 2, "conflicting contents")?;

    git.run_with_options(
        &["merge", "master"],
        &GitRunOptions {
            expected_exit_code: 1,
            ..Default::default()
        },
    )?;

    let glyphs = Glyphs::text();
    let effects = Effects::new_suppress_for_test(glyphs);
    let git_run_info = git.get_git_run_info();
    let repo = git.get_repo()?;
    let index = repo.get_index()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(SystemTime::now(), "testing")?;
    let head_info = repo.get_head_info()?;
    let (snapshot, status) = repo.get_status(
        &effects,
        &git_run_info,
        &index,
        &head_info,
        Some(event_tx_id),
    )?;
    insta::assert_debug_snapshot!(status, @r###"
        [
            StatusEntry {
                index_status: Unmerged,
                working_copy_status: Added,
                working_copy_file_mode: Blob,
                path: "test2.txt",
                orig_path: None,
            },
        ]
        "###);
    assert_eq!(
        snapshot.get_working_copy_changes_type()?,
        WorkingCopyChangesType::Conflicts
    );

    Ok(())
}
