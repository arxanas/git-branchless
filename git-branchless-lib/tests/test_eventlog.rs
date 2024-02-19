use std::str::FromStr;

use branchless::core::eventlog::testing::{new_event_cursor, new_event_transaction_id};
use branchless::core::eventlog::{
    testing::new_event_replayer, Event, EventLogDb, EventTransactionId,
};
use branchless::git::{MaybeZeroOid, NonZeroOid, ReferenceName};
use branchless::testing::make_git;

#[test]
fn test_drop_non_meaningful_events() -> eyre::Result<()> {
    let event_tx_id = new_event_transaction_id(123);
    let meaningful_event = Event::CommitEvent {
        timestamp: 0.0,
        event_tx_id,
        commit_oid: NonZeroOid::from_str("abc")?,
    };
    let mut replayer = new_event_replayer("refs/heads/master".into());
    replayer.process_event(&meaningful_event);
    replayer.process_event(&Event::RefUpdateEvent {
        timestamp: 0.0,
        event_tx_id,
        ref_name: ReferenceName::from("ORIG_HEAD"),
        old_oid: MaybeZeroOid::from_str("abc")?,
        new_oid: MaybeZeroOid::from_str("def")?,
        message: None,
    });
    replayer.process_event(&Event::RefUpdateEvent {
        timestamp: 0.0,
        event_tx_id,
        ref_name: ReferenceName::from("CHERRY_PICK_HEAD"),
        old_oid: MaybeZeroOid::Zero,
        new_oid: MaybeZeroOid::Zero,
        message: None,
    });

    let cursor = replayer.make_default_cursor();
    assert_eq!(
        replayer.get_event_before_cursor(cursor),
        Some((1, &meaningful_event))
    );
    Ok(())
}

#[test]
fn test_different_event_transaction_ids() -> eyre::Result<()> {
    let git = make_git()?;

    if git.produces_auto_merge_refs()? {
        return Ok(());
    }

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.branchless("hide", &["--no-delete-branches", "HEAD"])?;

    let repo = git.get_repo()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let events = event_log_db.get_events()?;
    let event_tx_ids: Vec<EventTransactionId> =
        events.iter().map(|event| event.get_event_tx_id()).collect();
    if git.supports_reference_transactions()? {
        insta::assert_debug_snapshot!(event_tx_ids, @r###"
        [
            Id(
                1,
            ),
            Id(
                1,
            ),
            Id(
                2,
            ),
            Id(
                3,
            ),
        ]
        "###);
    } else {
        insta::assert_debug_snapshot!(event_tx_ids, @r###"
        [
            Id(
                1,
            ),
            Id(
                2,
            ),
        ]
        "###);
    }
    Ok(())
}

#[test]
fn test_advance_cursor_by_transaction() -> eyre::Result<()> {
    let mut event_replayer = new_event_replayer("refs/heads/master".into());
    for (timestamp, event_tx_id) in (0..).zip(&[1, 1, 2, 2, 3, 4]) {
        let timestamp = f64::from(timestamp);
        event_replayer.process_event(&Event::UnobsoleteEvent {
            timestamp,
            event_tx_id: new_event_transaction_id(*event_tx_id),
            commit_oid: NonZeroOid::from_str("abc")?,
        });
    }

    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(0), 1),
        new_event_cursor(2),
    );
    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(1), 1),
        new_event_cursor(4),
    );
    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(2), 1),
        new_event_cursor(4),
    );
    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(3), 1),
        new_event_cursor(5),
    );
    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(4), 1),
        new_event_cursor(5),
    );
    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(5), 1),
        new_event_cursor(6),
    );
    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(6), 1),
        new_event_cursor(6),
    );

    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(6), -1),
        new_event_cursor(5),
    );
    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(5), -1),
        new_event_cursor(4),
    );
    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(4), -1),
        new_event_cursor(2),
    );
    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(3), -1),
        new_event_cursor(2),
    );
    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(2), -1),
        new_event_cursor(0),
    );
    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(1), -1),
        new_event_cursor(0),
    );
    assert_eq!(
        event_replayer.advance_cursor_by_transaction(new_event_cursor(0), -1),
        new_event_cursor(0),
    );

    Ok(())
}
