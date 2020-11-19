"""Handle hiding commits explicitly."""
import time
from typing import List, TextIO, Tuple

import pygit2

from . import get_repo
from .db import make_db_for_repo
from .eventlog import EventLogDb, EventReplayer, HideEvent, OidStr, UnhideEvent
from .formatting import Formatter


class CommitNotFoundError(Exception):
    def __init__(self, hash: str) -> None:
        self._hash = hash

    def __str__(self) -> str:
        return f"Commit not found: {self._hash}\n"


def _process_hashes(
    out: TextIO, repo: pygit2.Repository, hashes: List[str]
) -> Tuple[EventReplayer, EventLogDb, List[OidStr]]:
    db = make_db_for_repo(repo=repo)
    event_log_db = EventLogDb(db)

    replayer = EventReplayer()
    for event in event_log_db.get_events():
        replayer.process_event(event)

    oids = []
    for hash in hashes:
        try:
            oid = repo.revparse_single(hash).oid
        except KeyError as e:
            raise CommitNotFoundError(hash) from e
        oids.append(oid.hex)
    return (replayer, event_log_db, oids)


def hide(*, out: TextIO, hashes: List[str]) -> int:
    """Hide the hashes provided on the command-line.

    Args:
      out: The output stream to write to.
      hashes: A list of commit hashes to hide. Revs will be resolved (you can
        provide an abbreviated commit hash or ref name).

    Returns:
      Exit code (0 denotes successful exit).
    """
    timestamp = time.time()
    formatter = Formatter()
    repo = get_repo()
    try:
        (replayer, event_log_db, oids) = _process_hashes(
            out=out, repo=repo, hashes=hashes
        )
    except CommitNotFoundError as e:
        out.write(str(e))
        return 1
    events = [HideEvent(timestamp=timestamp, commit_oid=oid) for oid in oids]
    event_log_db.add_events(events)

    for event in events:
        oid = repo[event.commit_oid].oid
        out.write(formatter.format("Hid commit: {oid:oid}\n", oid=oid))
        if replayer.get_commit_visibility(oid.hex) == "hidden":
            out.write("(It was already hidden, so this operation had no effect.)\n")
        out.write(
            formatter.format(
                "To unhide this commit, run: git unhide {oid:oid}\n", oid=oid
            )
        )
    return 0


def unhide(*, out: TextIO, hashes: List[str]) -> int:
    """Unhide the hashes provided on the command-line.

    Args:
      out: The output stream to write to.
      hashes: A list of commit hashes to unhide. Revs will be resolved (you
        can provide an abbreviated commit hash or ref name).

    Returns:
      Exit code (0 denotes successful exit).
    """
    timestamp = time.time()
    formatter = Formatter()
    repo = get_repo()
    try:
        (replayer, event_log_db, oids) = _process_hashes(
            repo=repo, out=out, hashes=hashes
        )
    except CommitNotFoundError as e:
        out.write(str(e))
        return 1
    events = [UnhideEvent(timestamp=timestamp, commit_oid=oid) for oid in oids]
    event_log_db.add_events(events)

    for event in events:
        oid = repo[event.commit_oid].oid
        out.write(formatter.format("Unhid commit: {oid:oid}\n", oid=oid))
        if replayer.get_commit_visibility(oid.hex) == "visible":
            out.write("(It was not hidden, so this operation had no effect.)\n")
        out.write(
            formatter.format("To hide this commit, run: git hide {oid:oid}\n", oid=oid)
        )
    return 0
