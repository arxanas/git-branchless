use branchless::core::eventlog::testing::{get_event_replayer_events, redact_event_timestamp};
use branchless::core::eventlog::{Event, EventLogDb, EventReplayer};
use branchless::testing::make_git;
use branchless::util::get_db_conn;

#[test]
fn test_git_v2_31_events() -> anyhow::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.run(&["checkout", "-b", "test1"])?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file("test2", 2)?;
    git.run(&["hide", "test1"])?;
    git.run(&["branch", "-D", "test1"])?;

    let conn = get_db_conn(&*(git.get_repo()?))?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
    let events: Vec<Event> = get_event_replayer_events(&event_replayer)
        .iter()
        .cloned()
        .map(redact_event_timestamp)
        .collect();
    insta::assert_debug_snapshot!(events, @r###"
            [
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        1,
                    ),
                    ref_name: "refs/heads/test1",
                    old_ref: None,
                    new_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    message: None,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        2,
                    ),
                    ref_name: "HEAD",
                    old_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    new_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    message: None,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        3,
                    ),
                    ref_name: "HEAD",
                    old_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    new_ref: Some(
                        "62fc20d2a290daea0d52bdc2ed2ad4be6491010e",
                    ),
                    message: None,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        3,
                    ),
                    ref_name: "refs/heads/test1",
                    old_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    new_ref: Some(
                        "62fc20d2a290daea0d52bdc2ed2ad4be6491010e",
                    ),
                    message: None,
                },
                CommitEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        4,
                    ),
                    commit_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        5,
                    ),
                    ref_name: "HEAD",
                    old_ref: None,
                    new_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    message: None,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        6,
                    ),
                    ref_name: "HEAD",
                    old_ref: Some(
                        "62fc20d2a290daea0d52bdc2ed2ad4be6491010e",
                    ),
                    new_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    message: None,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        7,
                    ),
                    ref_name: "HEAD",
                    old_ref: Some(
                        "f777ecc9b0db5ed372b2615695191a8a17f79f24",
                    ),
                    new_ref: Some(
                        "fe65c1fe15584744e649b2c79d4cf9b0d878f92e",
                    ),
                    message: None,
                },
                CommitEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        8,
                    ),
                    commit_oid: fe65c1fe15584744e649b2c79d4cf9b0d878f92e,
                },
                HideEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        9,
                    ),
                    commit_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                },
                RefUpdateEvent {
                    timestamp: 0.0,
                    event_tx_id: EventTransactionId(
                        10,
                    ),
                    ref_name: "refs/heads/test1",
                    old_ref: Some(
                        "62fc20d2a290daea0d52bdc2ed2ad4be6491010e",
                    ),
                    new_ref: None,
                    message: None,
                },
            ]
            "###);

    Ok(())
}
