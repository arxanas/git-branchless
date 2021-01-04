from branchless.eventlog import CommitEvent, EventReplayer, RefUpdateEvent


def test_drop_non_meaningful_events() -> None:
    meaningful_event = CommitEvent(timestamp=1.0, commit_oid="abc")
    replayer = EventReplayer()
    replayer.process_event(meaningful_event)
    replayer.process_event(
        RefUpdateEvent(
            timestamp=2.0,
            ref_name="ORIG_HEAD",
            old_ref="abc",
            new_ref="def",
            message=None,
        )
    )
    replayer.process_event(
        RefUpdateEvent(
            timestamp=3.0,
            ref_name="CHERRY_PICK_HEAD",
            old_ref=None,
            new_ref=None,
            message=None,
        )
    )
    result = replayer.get_event_before_cursor()
    assert result is not None
    (event_id, event) = result
    assert event_id == 1
    assert event.timestamp == 1.0
