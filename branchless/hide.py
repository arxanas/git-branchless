"""Handle hiding commits explicitly."""
from typing import TextIO, List
import time


from . import get_repo
from .db import make_db_for_repo
from .eventlog import EventLogDb, EventReplayer, HideEvent
from .formatting import Formatter


def hide(*, out: TextIO, hashes: List[str]) -> int:
    """Hide the hashes provided on the command-line.

    Args:
      out: The output stream to write to.
      hashes: A list of commit hashes to hide. Revs will be resolved (you can
        provide an abbreviated commit hash or ref name).

    Returns:
      Exit code (0 denotes successful exit).
    """
    formatter = Formatter()
    repo = get_repo()
    db = make_db_for_repo(repo=repo)
    event_log_db = EventLogDb(db)

    replayer = EventReplayer()
    for event in event_log_db.get_events():
        replayer.process_event(event)

    timestamp = time.time()
    events = []
    for hash in hashes:
        try:
            oid = repo.revparse_single(hash).oid
        except KeyError:
            out.write(f"Commit not found: {hash}\n")
            return 1
        events.append(HideEvent(timestamp=timestamp, commit_oid=oid.hex))

    event_log_db.add_events(events)

    for event in events:
        oid = repo[event.commit_oid].oid
        out.write(formatter.format("Hid commit: {oid:oid}\n", oid=oid))
        if not replayer.is_commit_visible(oid.hex):
            out.write("(It was already hidden, so this operation had no effect.)\n")
        out.write(
            formatter.format(
                "To unhide this commit, run: git checkout {oid:oid}\n", oid=oid
            )
        )
    return 0
