from branchless.eventlog import CommitEvent, EventReplayer, RefUpdateEvent


def test_drop_non_meaningful_events() -> None:
    meaningful_event = CommitEvent(timestamp=0.0, commit_oid="abc")
    replayer = EventReplayer()
    replayer.process_event(meaningful_event)
    replayer.process_event(
        RefUpdateEvent(
            timestamp=0.0,
            ref_name="ORIG_HEAD",
            old_ref="abc",
            new_ref="def",
            message=None,
        )
    )
    replayer.process_event(
        RefUpdateEvent(
            timestamp=0.0,
            ref_name="CHERRY_PICK_HEAD",
            old_ref=None,
            new_ref=None,
            message=None,
        )
    )
    assert replayer.get_event_before_cursor() == (1, meaningful_event)
