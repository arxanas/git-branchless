use lib::testing::make_git;
use lib::core::effects::Effects;
use lib::core::eventlog::testing::{get_event_replayer_events, redact_event_timestamp};
use lib::core::eventlog::{Event, EventLogDb, EventReplayer};
use lib::core::formatting::Glyphs;

#[test]
fn test_git_v2_31_events() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.run(&["checkout", "-b", "test1"])?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "HEAD^"])?;
    git.commit_file("test2", 2)?;
    git.branchless("hide", &["test1"])?;
    git.run(&["branch", "-D", "test1"])?;

    let effects = Effects::new_suppress_for_test(Glyphs::text());
    let repo = git.get_repo()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&effects, &repo, &event_log_db)?;
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
            ref_name: ReferenceName(
                "refs/heads/test1",
            ),
            old_oid: 0000000000000000000000000000000000000000,
            new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            message: None,
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                2,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            message: None,
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                3,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            message: None,
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                3,
            ),
            ref_name: ReferenceName(
                "refs/heads/test1",
            ),
            old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            new_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            message: None,
        },
        CommitEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                4,
            ),
            commit_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                5,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: 0000000000000000000000000000000000000000,
            new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            message: None,
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                6,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            new_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            message: None,
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                7,
            ),
            ref_name: ReferenceName(
                "HEAD",
            ),
            old_oid: f777ecc9b0db5ed372b2615695191a8a17f79f24,
            new_oid: fe65c1fe15584744e649b2c79d4cf9b0d878f92e,
            message: None,
        },
        CommitEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                8,
            ),
            commit_oid: NonZeroOid(fe65c1fe15584744e649b2c79d4cf9b0d878f92e),
        },
        ObsoleteEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                9,
            ),
            commit_oid: NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e),
        },
        RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id: EventTransactionId(
                10,
            ),
            ref_name: ReferenceName(
                "refs/heads/test1",
            ),
            old_oid: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
            new_oid: 0000000000000000000000000000000000000000,
            message: None,
        },
    ]
    "###);

    Ok(())
}
