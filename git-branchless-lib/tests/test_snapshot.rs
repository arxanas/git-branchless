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
    insta::assert_debug_snapshot!(status, @r#"
    [
        StatusEntry {
            index_status: Unmerged,
            working_copy_status: Added,
            working_copy_file_mode: Blob,
            path: "test2.txt",
            orig_path: None,
        },
    ]
    "#);
    assert_eq!(
        snapshot.get_working_copy_changes_type()?,
        WorkingCopyChangesType::Conflicts
    );

    Ok(())
}

/// If a *tracked* file becomes unreadable (e.g. its permissions were
/// stripped), `Repo::get_status` should fail with a clear error identifying
/// the offending path and suggesting that the user check its permissions,
/// rather than panicking or silently producing a partial snapshot that
/// omits the unreadable file.
///
/// See https://github.com/arxanas/git-branchless/issues/1658
#[cfg(unix)]
#[test]
fn test_get_status_unreadable_tracked_file() -> eyre::Result<()> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let git = make_git()?;
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.write_file_txt("test1", "test1 new contents\n")?;

    let file_path = git.repo_path.join("test1.txt");
    fs::set_permissions(&file_path, fs::Permissions::from_mode(0o000))?;

    // Root ignores Unix file permission bits, so this test can't validate
    // anything meaningful when run as root (e.g. in some Docker setups).
    // Detect that case and skip rather than fail confusingly.
    let running_as_root = fs::read(&file_path).is_ok();
    let message: Option<String> = if running_as_root {
        None
    } else {
        let glyphs = Glyphs::text();
        let effects = Effects::new_suppress_for_test(glyphs);
        let git_run_info = git.get_git_run_info();
        let repo = git.get_repo()?;
        let index = repo.get_index()?;
        let conn = repo.get_db_conn()?;
        let event_log_db = EventLogDb::new(&conn)?;
        let event_tx_id = event_log_db.make_transaction_id(SystemTime::now(), "testing")?;
        let head_info = repo.get_head_info()?;
        let result = repo.get_status(
            &effects,
            &git_run_info,
            &index,
            &head_info,
            Some(event_tx_id),
        );
        match result {
            Ok(_) => None,
            Err(err) => Some(format!("{err:#}")),
        }
    };

    // Restore permissions so that the temp dir can be cleaned up afterwards.
    fs::set_permissions(&file_path, fs::Permissions::from_mode(0o644))?;

    if running_as_root {
        return Ok(());
    }

    let message = message.expect("expected get_status to fail for an unreadable tracked file");
    assert!(
        message.contains("could not create blob from"),
        "unexpected error message: {message}"
    );
    assert!(
        message.contains("test1.txt"),
        "error message should include the offending path: {message}"
    );
    // The raw OS error text ("Permission denied (os error 13)") already
    // mentions "permission", so assert on the project-added, actionable
    // suggestion specifically (distinct from the raw OS error string) to
    // make sure we're actually testing our diagnostic, not just libc's.
    assert!(
        message.to_lowercase().contains("chmod"),
        "error message should suggest fixing permissions (e.g. via chmod): {message}"
    );

    Ok(())
}

/// An *untracked* unreadable file should never cause `Repo::get_status` (and
/// therefore snapshot creation) to fail, because untracked files are
/// excluded from status/snapshots by design (`--untracked-files=no`). This
/// pins that invariant, per the report thread's request to also cover the
/// untracked case.
///
/// See https://github.com/arxanas/git-branchless/issues/1658
#[cfg(unix)]
#[test]
fn test_get_status_unreadable_untracked_file() -> eyre::Result<()> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let git = make_git()?;
    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.write_file_txt("test1", "test1 new contents\n")?;
    git.write_file_txt("untracked", "untracked contents\n")?;

    let file_path = git.repo_path.join("untracked.txt");
    fs::set_permissions(&file_path, fs::Permissions::from_mode(0o000))?;

    let running_as_root = fs::read(&file_path).is_ok();
    let status: Option<Result<Vec<branchless::git::StatusEntry>, _>> = if running_as_root {
        None
    } else {
        let glyphs = Glyphs::text();
        let effects = Effects::new_suppress_for_test(glyphs);
        let git_run_info = git.get_git_run_info();
        let repo = git.get_repo()?;
        let index = repo.get_index()?;
        let conn = repo.get_db_conn()?;
        let event_log_db = EventLogDb::new(&conn)?;
        let event_tx_id = event_log_db.make_transaction_id(SystemTime::now(), "testing")?;
        let head_info = repo.get_head_info()?;
        let result = repo.get_status(
            &effects,
            &git_run_info,
            &index,
            &head_info,
            Some(event_tx_id),
        );
        Some(result.map(|(_snapshot, status)| status))
    };

    fs::set_permissions(&file_path, fs::Permissions::from_mode(0o644))?;

    if running_as_root {
        return Ok(());
    }

    use std::path::Path;
    let status = status.expect("guarded by running_as_root check above")?;
    assert!(
        status
            .iter()
            .all(|entry| entry.path != Path::new("untracked.txt")),
        "untracked files should never be included in status/snapshots: {status:?}"
    );

    Ok(())
}
