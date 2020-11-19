"""Process our event log.

We use Git hooks to record the actions that the user takes over time, and put
them in persistent storage. Later, we play back the actions in order to
determine what actions the user took on the repository, and which commits
they're still working on.
"""
import collections
import enum
import sqlite3
import sys
import time
from dataclasses import dataclass
from typing import Dict, List, Optional, Set, TextIO, Tuple, Union

from . import get_repo
from .db import make_cursor, make_db_for_repo

OidStr = str
"""Represents an object ID in the Git repository.

We don't use `pygit2.Oid` directly since it requires looking up the object in
the repo, and we don't want to spend time hitting disk for that.
Consequently, the object pointed to by an OID is not guaranteed to exist
anymore (such as if it was garbage collected).
"""


@dataclass(frozen=True, eq=True)
class _BaseEvent:
    timestamp: float


@dataclass(frozen=True, eq=True)
class _RewriteEvent(_BaseEvent):
    type = "rewrite"
    commit_oid_old: OidStr
    commit_oid_new: OidStr


@dataclass(frozen=True, eq=True)
class _RefUpdateEvent(_BaseEvent):
    type = "ref-move"
    ref_name: str
    ref_old: str
    ref_new: str
    message: Optional[str]


@dataclass(frozen=True, eq=True)
class _CommitEvent(_BaseEvent):
    type = "commit"
    commit_oid: OidStr


@dataclass(frozen=True, eq=True)
class _HideEvent(_BaseEvent):
    type = "hide"
    commit_oid: OidStr


@dataclass(frozen=True, eq=True)
class _UnhideEvent(_BaseEvent):
    type = "unhide"
    commit_oid: str


Event = Union[_RewriteEvent, _RefUpdateEvent, _CommitEvent, _HideEvent, _UnhideEvent]
"""One of the possible events that the user took."""


class EventLogDb:
    """Stores `Event`s on disk."""

    @dataclass(frozen=True, eq=True)
    class Row:
        """Wrapper around the row stored directly in the database."""

        timestamp: float
        type: str
        ref1: Optional[str]
        ref2: Optional[str]
        ref_name: Optional[str]
        message: Optional[str]

    def __init__(self, conn: sqlite3.Connection) -> None:
        """Constructor.

        Args:
          conn: The database connection.
        """
        self._conn = conn
        self._init_tables()

    def _init_tables(self) -> None:
        with make_cursor(self._conn) as cursor:
            cursor.execute(
                """
CREATE TABLE IF NOT EXISTS event_log (
    timestamp REAL NOT NULL,
    type TEXT NOT NULL,
    ref_old TEXT,
    ref_new TEXT,
    ref_name TEXT,
    message TEXT
)
"""
            )

    def add_events(
        self,
        rows: List[Row],
    ) -> None:
        """Add events in the given order to the database, in a transaction.

        Args:
          rows: The rows to add.
        """
        with make_cursor(self._conn) as cursor:
            for row in rows:
                cursor.execute(
                    """
INSERT INTO event_log VALUES (
    :timestamp,
    :type,
    :ref_old,
    :ref_new,
    :ref_name,
    :message
)
""",
                    {
                        "timestamp": row.timestamp,
                        "type": row.type,
                        "ref_old": row.ref1,
                        "ref_new": row.ref2,
                        "ref_name": row.ref_name,
                        "message": row.message,
                    },
                )

    def get_events(self) -> List[Event]:
        """Get all the events in the database.

        Returns:
          All the events in the database, ordered from oldest to newest.
        """
        with make_cursor(self._conn) as cursor:
            result = cursor.execute(
                """
SELECT timestamp, type, ref_old, ref_new, ref_name, message
FROM event_log
ORDER BY timestamp ASC,
         rowid ASC
"""
            )

        rows = result.fetchall()
        events: List[Event] = []
        for (timestamp, type, ref1, ref2, ref_name, message) in rows:
            if type == _RewriteEvent.type:
                events.append(
                    _RewriteEvent(
                        timestamp=timestamp, commit_oid_old=ref1, commit_oid_new=ref2
                    )
                )
            elif type == _RefUpdateEvent.type:
                events.append(
                    _RefUpdateEvent(
                        timestamp=timestamp,
                        ref_name=ref_name,
                        ref_old=ref1,
                        ref_new=ref2,
                        message=message,
                    )
                )
            elif type == _CommitEvent.type:
                events.append(_CommitEvent(timestamp=timestamp, commit_oid=ref1))
            elif type == _HideEvent.type:
                events.append(_HideEvent(timestamp=timestamp, commit_oid=ref1))
            elif type == _UnhideEvent.type:
                events.append(_UnhideEvent(timestamp=timestamp, commit_oid=ref1))
            else:
                raise TypeError(f"Unknown event log type: {type}")

        return events


def hook_post_rewrite(out: TextIO) -> None:
    """Handle Git's post-rewrite hook.

    Args:
      out: Output stream to write to.
    """
    timestamp = time.time()
    events = []
    for line in sys.stdin:
        line = line.strip()
        [ref_old, ref_new, *extras] = line.split(" ")
        events.append(
            EventLogDb.Row(
                timestamp=timestamp,
                type=_RewriteEvent.type,
                ref1=ref_old,
                ref2=ref_new,
                ref_name=None,
                message=None,
            )
        )
    out.write(f"branchless: processing {len(events)} rewritten commit(s)\n")

    repo = get_repo()
    db = make_db_for_repo(repo=repo)
    event_log_db = EventLogDb(db)
    event_log_db.add_events(events)


def hook_post_checkout(
    out: TextIO, previous_head_ref: str, current_head_ref: str, is_branch_checkout: int
) -> None:
    """Handle Git's post-checkout hook.

    Args:
      out: Output stream to write to.
    """
    if is_branch_checkout == 0:
        return

    timestamp = time.time()
    out.write("branchless: processing checkout\n")

    repo = get_repo()
    db = make_db_for_repo(repo=repo)
    event_log_db = EventLogDb(db)
    event_log_db.add_events(
        [
            EventLogDb.Row(
                timestamp=timestamp,
                type=_RefUpdateEvent.type,
                ref1=previous_head_ref,
                ref2=current_head_ref,
                ref_name="HEAD",
                message=None,
            )
        ]
    )


def hook_post_commit(out: TextIO) -> None:
    """Handle Git's post-commit hook.

    Args:
      out: Output stream to write to.
    """
    timestamp = time.time()
    out.write("branchless: processing commit\n")

    repo = get_repo()
    db = make_db_for_repo(repo=repo)
    event_log_db = EventLogDb(db)
    event_log_db.add_events(
        [
            EventLogDb.Row(
                timestamp=timestamp,
                type=_CommitEvent.type,
                ref1=repo.head.target.hex,
                ref2=None,
                ref_name=None,
                message=None,
            )
        ]
    )


class _EventClassification(enum.Enum):
    SHOW = enum.auto()
    HIDE = enum.auto()


class EventReplayer:
    """Processes events in order and determine the repo's visible commits."""

    def __init__(self) -> None:
        self._commit_history: Dict[
            str, List[Tuple[_EventClassification, Event]]
        ] = collections.defaultdict(list)

    def process_event(self, event: Event) -> None:
        """Process the given event.

        Args:
          event: The next event to process. Events should be passed to the
          replayer in order from oldest to newest.
        """
        if isinstance(event, _RewriteEvent):
            self._commit_history[event.commit_oid_old].append(
                (_EventClassification.HIDE, event)
            )
            self._commit_history[event.commit_oid_new].append(
                (_EventClassification.SHOW, event)
            )
        elif isinstance(event, _RefUpdateEvent):
            # Currently, we don't process this.
            pass
        elif isinstance(event, _CommitEvent):
            self._commit_history[event.commit_oid].append(
                (_EventClassification.SHOW, event)
            )
        elif isinstance(event, _HideEvent):
            self._commit_history[event.commit_oid].append(
                (_EventClassification.HIDE, event)
            )
        elif isinstance(event, _UnhideEvent):
            self._commit_history[event.commit_oid].append(
                (_EventClassification.SHOW, event)
            )
        else:
            raise TypeError(f"Unhandled event: {event}")

    def get_visible_oids(self) -> Set[str]:
        """Get the visible OIDs according to the repository history.

        Returns:
          The set of OIDs referring to commits which are thought to be
          visible due to user action.
        """
        return {
            oid
            for oid, history in self._commit_history.items()
            if history[-1][0] is _EventClassification.SHOW
        }
