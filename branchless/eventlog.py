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
from typing import Dict, List, Optional, Sequence, Set, TextIO, Tuple, Union

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
class _Row:
    """Wrapper around the row stored directly in the database."""

    timestamp: float
    type: str
    ref1: Optional[str]
    ref2: Optional[str]
    ref_name: Optional[str]
    message: Optional[str]


@dataclass(frozen=True, eq=True)
class _BaseEvent:
    timestamp: float

    def to_row(self) -> _Row:
        raise NotImplementedError()

    @classmethod
    def from_row(cls, row: _Row) -> "Event":
        raise NotImplementedError()


@dataclass(frozen=True, eq=True)
class RewriteEvent(_BaseEvent):
    """Indicates that the commit was rewritten.

    Examples of rewriting include rebases and amended commits.

    We typically want to mark the new version of the commit as visible and
    the old version of the commit as hidden.
    """

    type = "rewrite"

    commit_oid_old: OidStr
    """The OID of the commit before the rewrite."""

    commit_oid_new: OidStr
    """The OID of the commit after the rewrite."""

    def to_row(self) -> _Row:
        return _Row(
            timestamp=self.timestamp,
            type=self.type,
            ref1=self.commit_oid_old,
            ref2=self.commit_oid_new,
            ref_name=None,
            message=None,
        )

    @classmethod
    def from_row(cls, row: _Row) -> "RewriteEvent":
        assert row.type == cls.type
        assert row.ref1 is not None
        assert row.ref2 is not None
        return cls(
            timestamp=row.timestamp, commit_oid_old=row.ref1, commit_oid_new=row.ref2
        )


@dataclass(frozen=True, eq=True)
class RefUpdateEvent(_BaseEvent):
    """Indicates that a reference was updated.

    The most important reference we track is HEAD. In principle, we can also
    track branch moves in this way, but Git doesn't support the appropriate
    hook until v2.29 (`reference-transaction`).
    """

    type = "ref-move"

    ref_name: str
    """The full name of the reference that was updated.

    For example, `HEAD` or `refs/heads/master`.
    """

    ref_old: str
    """The old referent.

    May be an OID (in the case of a direct reference) or another reference
    name (in the case of a symbolic reference).
    """

    ref_new: str
    """The updated referent.

    This may not be different from the old referent.

    May be an OID (in the case of a direct reference) or another reference
    name (in the case of a symbolic reference).
    """

    message: Optional[str]
    """A message associated with the rewrite, if any."""

    def to_row(self) -> _Row:
        return _Row(
            timestamp=self.timestamp,
            type=self.type,
            ref1=self.ref_old,
            ref2=self.ref_new,
            ref_name=self.ref_name,
            message=self.message,
        )

    @classmethod
    def from_row(cls, row: _Row) -> "RefUpdateEvent":
        assert row.type == cls.type
        assert row.ref1 is not None
        assert row.ref2 is not None
        assert row.ref_name is not None
        return cls(
            timestamp=row.timestamp,
            ref_old=row.ref1,
            ref_new=row.ref2,
            ref_name=row.ref_name,
            message=row.message,
        )


@dataclass(frozen=True, eq=True)
class CommitEvent(_BaseEvent):
    """Indicate that the user made a commit.

    User commits should be marked as visible.
    """

    type = "commit"

    commit_oid: OidStr
    """The new commit OID."""

    def to_row(self) -> _Row:
        return _Row(
            timestamp=self.timestamp,
            type=self.type,
            ref1=self.commit_oid,
            ref2=None,
            ref_name=None,
            message=None,
        )

    @classmethod
    def from_row(cls, row: _Row) -> "CommitEvent":
        assert row.type == cls.type
        assert row.ref1 is not None
        return cls(
            timestamp=row.timestamp,
            commit_oid=row.ref1,
        )


@dataclass(frozen=True, eq=True)
class HideEvent(_BaseEvent):
    """Indicates that a commit was explicitly hidden by the user.

    If the commit in question was not already visible, then this has no
    practical effect.
    """

    type = "hide"

    commit_oid: OidStr
    """The OID of the commit that was hidden."""

    def to_row(self) -> _Row:
        return _Row(
            timestamp=self.timestamp,
            type=self.type,
            ref1=self.commit_oid,
            ref2=None,
            ref_name=None,
            message=None,
        )

    @classmethod
    def from_row(cls, row: _Row) -> "HideEvent":
        assert row.type == cls.type
        assert row.ref1 is not None
        return cls(
            timestamp=row.timestamp,
            commit_oid=row.ref1,
        )


@dataclass(frozen=True, eq=True)
class UnhideEvent(_BaseEvent):
    """Indicates that a commit was explicitly un-hidden by the user.

    If the commit in question was not already hidden, then this has no
    practical effect.
    """

    type = "unhide"

    commit_oid: str
    """The OID of the commit that was unhidden."""

    def to_row(self) -> _Row:
        return _Row(
            timestamp=self.timestamp,
            type=self.type,
            ref1=self.commit_oid,
            ref2=None,
            ref_name=None,
            message=None,
        )

    @classmethod
    def from_row(cls, row: _Row) -> "UnhideEvent":
        assert row.type == cls.type
        assert row.ref1 is not None
        return cls(
            timestamp=row.timestamp,
            commit_oid=row.ref1,
        )


Event = Union[RewriteEvent, RefUpdateEvent, CommitEvent, HideEvent, UnhideEvent]
"""One of the possible events that the user took."""


class EventLogDb:
    """Stores `Event`s on disk."""

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
        events: Sequence[Event],
    ) -> None:
        """Add events in the given order to the database, in a transaction.

        Args:
          events: The events to add.
        """
        with make_cursor(self._conn) as cursor:
            for event in events:
                row = event.to_row()
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

    def get_events(self) -> Sequence[Event]:
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
            row = _Row(
                timestamp=timestamp,
                type=type,
                ref1=ref1,
                ref2=ref2,
                ref_name=ref_name,
                message=message,
            )
            if type == RewriteEvent.type:
                events.append(RewriteEvent.from_row(row))
            elif type == RefUpdateEvent.type:
                events.append(RefUpdateEvent.from_row(row))
            elif type == CommitEvent.type:
                events.append(CommitEvent.from_row(row))
            elif type == HideEvent.type:
                events.append(HideEvent.from_row(row))
            elif type == UnhideEvent.type:
                events.append(UnhideEvent.from_row(row))
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
            RewriteEvent(
                timestamp=timestamp, commit_oid_old=ref_old, commit_oid_new=ref_new
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
            RefUpdateEvent(
                timestamp=timestamp,
                ref_old=previous_head_ref,
                ref_new=current_head_ref,
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
            CommitEvent(
                timestamp=timestamp,
                commit_oid=repo.head.target.hex,
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
        if isinstance(event, RewriteEvent):
            self._commit_history[event.commit_oid_old].append(
                (_EventClassification.HIDE, event)
            )
            self._commit_history[event.commit_oid_new].append(
                (_EventClassification.SHOW, event)
            )
        elif isinstance(event, RefUpdateEvent):
            # Currently, we don't process this.
            pass
        elif isinstance(event, CommitEvent):
            self._commit_history[event.commit_oid].append(
                (_EventClassification.SHOW, event)
            )
        elif isinstance(event, HideEvent):
            self._commit_history[event.commit_oid].append(
                (_EventClassification.HIDE, event)
            )
        elif isinstance(event, UnhideEvent):
            self._commit_history[event.commit_oid].append(
                (_EventClassification.SHOW, event)
            )
        else:
            raise TypeError(f"Unhandled event: {event}")

    def is_commit_visible(self, oid: OidStr) -> Optional[bool]:
        """Determines whether a commit has been marked as visible.

        Args:
          oid: The OID of the commit to check.

        Returns:
          Whether or not the commit is visible. Returns `None` if no history
          has been recorded for that commit.
        """
        if oid not in self._commit_history:
            return None
        (classification, history) = self._commit_history[oid][-1]
        return classification is _EventClassification.SHOW

    def get_visible_oids(self) -> Set[str]:
        """Get the visible OIDs according to the repository history.

        Returns:
          The set of OIDs referring to commits which are thought to be visible due to user action.
        """
        return {
            oid
            for oid, history in self._commit_history.items()
            if history[-1][0] is _EventClassification.SHOW
        }

    def get_hidden_oids(self) -> Set[str]:
        """Get the hidden OIDs according to the repository history.

        Returns:
          The set of OIDs referring to commits which are thought to be hidden due to user action.
        """
        return {
            oid
            for oid, history in self._commit_history.items()
            if history[-1][0] is _EventClassification.HIDE
        }
