import sqlite3
from dataclasses import dataclass
from typing import Dict, List, Optional, Sequence, Set, Tuple, Union

import pygit2
from typing_extensions import Literal

from . import OidStr

class PyMergeBaseDb:
    """Cache for merge-base queries."""

    def __init__(self, conn: sqlite3.Connection) -> None:
        """Constructor.

        Args:
          conn: The database connection.
        """
        ...
    def get_merge_base_oid(
        self, repo: pygit2.Repository, lhs_oid: pygit2.Oid, rhs_oid: pygit2.Oid
    ) -> Optional[pygit2.Oid]:
        """Get the merge-base for two given commits.

        If the query is already in the cache, return the cached result. If
        not, it is computed, cached, and returned.

        Args:
          repo: The Git repo.
          lhs_oid: The first OID (ordering is arbitrary).
          rhs_oid: The second OID (ordering is arbitrary).

        Returns:
          The merge-base OID for these two commits. Returns `None` if no
          merge-base could be found.
        """
        ...

@dataclass(frozen=True, eq=True)
class _BaseEvent:
    timestamp: float

@dataclass(frozen=True, eq=True)
class PyRewriteEvent(_BaseEvent):
    """Indicates that the commit was rewritten.

    Examples of rewriting include rebases and amended commits.

    We typically want to mark the new version of the commit as visible and
    the old version of the commit as hidden.
    """

    type = "rewrite"

    old_commit_oid: OidStr
    """The OID of the commit before the rewrite."""

    new_commit_oid: OidStr
    """The OID of the commit after the rewrite."""

@dataclass(frozen=True, eq=True)
class PyRefUpdateEvent(_BaseEvent):
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

    old_ref: Optional[str]
    """The old referent.

    May be an OID (in the case of a direct reference) or another reference
    name (in the case of a symbolic reference).
    """

    new_ref: Optional[str]
    """The updated referent.

    This may not be different from the old referent.

    May be an OID (in the case of a direct reference) or another reference
    name (in the case of a symbolic reference).
    """

    message: Optional[str]
    """A message associated with the rewrite, if any."""

@dataclass(frozen=True, eq=True)
class PyCommitEvent(_BaseEvent):
    """Indicate that the user made a commit.

    User commits should be marked as visible.
    """

    type = "commit"

    commit_oid: OidStr
    """The new commit OID."""

@dataclass(frozen=True, eq=True)
class PyHideEvent(_BaseEvent):
    """Indicates that a commit was explicitly hidden by the user.

    If the commit in question was not already visible, then this has no
    practical effect.
    """

    type = "hide"

    commit_oid: OidStr
    """The OID of the commit that was hidden."""

@dataclass(frozen=True, eq=True)
class PyUnhideEvent(_BaseEvent):
    """Indicates that a commit was explicitly un-hidden by the user.

    If the commit in question was not already hidden, then this has no
    practical effect.
    """

    type = "unhide"

    commit_oid: str
    """The OID of the commit that was unhidden."""

Event = Union[
    PyRewriteEvent, PyRefUpdateEvent, PyCommitEvent, PyHideEvent, PyUnhideEvent
]
"""One of the possible events that the user took."""

class PyEventLogDb:
    """Stores `Event`s on disk."""

    def __init__(self, conn: sqlite3.Connection) -> None:
        """Constructor.

        Args:
          conn: The database connection.
        """
        ...
    def _init_tables(self) -> None: ...
    def add_events(
        self,
        events: Sequence[Event],
    ) -> None:
        """Add events in the given order to the database, in a transaction.

        Args:
          events: The events to add.
        """
        ...
    def get_events(self) -> Sequence[Event]:
        """Get all the events in the database.

        Returns:
          All the events in the database, ordered from oldest to newest.
        """
        ...

class PyEventReplayer:
    """Processes events in order and determine the repo's visible commits."""

    def __init__(self) -> None: ...
    @classmethod
    def from_event_log_db(cls, event_log_db: PyEventLogDb) -> "PyEventReplayer":
        """Construct the replayer from all the events in the database.

        Args:
          event_log_db: The database to query events from.

        Returns:
          The constructed replayer.
        """
        ...
    def process_event(self, event: Event) -> None:
        """Process the given event.

        This also sets the event cursor to point to immediately after the
        event that was just processed.

        Args:
          event: The next event to process. Events should be passed to the
          replayer in order from oldest to newest.
        """
        ...
    def get_commit_visibility(
        self, oid: OidStr
    ) -> Optional[Union[Literal["visible"], Literal["hidden"]]]:
        """Determines whether a commit has been marked as visible or hidden.

        Args:
          oid: The OID of the commit to check.

        Returns:
          Whether the commit is visible or hidden. Returns `None` if no history
          has been recorded for that commit.
        """
        ...
    def get_commit_latest_event(self, oid: OidStr) -> Optional[Event]:
        """Get the latest event affecting a given commit.

        Args:
          oid: The OID of the commit to check.

        Returns:
          The most recent event that affected that commit. If this commit was
          not observed by the replayer, returns `None`.
        """
        ...
    def get_active_oids(self) -> Set[str]:
        """Get the OIDs which have activity according to the repository history.

        Returns:
          The set of OIDs referring to commits which are thought to be active
          due to user action.
        """
        ...
    def set_cursor(self, event_id: int) -> None:
        """Set the event cursor to point to immediately after the provided event.

        The "event cursor" is used to move the event replayer forward or
        backward in time, so as to show the state of the repository at that
        time.

        The cursor is a position in between two events in the event log.
        Thus, all events before to the cursor are considered to be in effect,
        and all events after the cursor are considered to not have happened
        yet.

        Args:
          event_id: The index of the event to set the cursor to point
            immediately after. If out of bounds, the cursor is set to the
            first or last valid position, as appropriate.
        """
        ...
    def advance_cursor(self, num_events: int) -> None:
        """Advance the event cursor by the specified number of events.

        Args:
          num_events: The number of events to advance by. Can be positive,
            zero, or negative. If out of bounds, the cursor is set to the
            first or last valid position, as appropriate.
        """
        ...
    def get_cursor_head_oid(self) -> Optional[OidStr]:
        """Get the OID of `HEAD` at the cursor's point in time.

        Returns:
          The OID pointed to by `HEAD` at that time, or `None` if `HEAD` was
          never observed.
        """
        ...
    def get_cursor_main_branch_oid(self, repo: pygit2.Repository) -> pygit2.Oid: ...
    def get_cursor_branch_oid_to_names(
        self, repo: pygit2.Repository
    ) -> Dict[OidStr, Set[str]]:
        """Get the mapping of branch OIDs to names at the cursor's point in
        time.

        Same as `branchless.get_branch_oid_to_names`, but for a previous
        point in time.

        Args:
          repo: The Git repository.

        Returns:
          A mapping from an OID to the names of branches pointing to that
          OID.
        """
        ...
    def get_event_before_cursor(self) -> Optional[Tuple[int, Event]]:
        """Get the event immediately before the cursor.

        Returns:
          A tuple of event ID and the event that most recently happened. If
          no event was before the event cursor, returns `None` instead.
        """
        ...
    def get_events_since_cursor(self) -> List[Event]:
        """Get all the events that have happened since the event cursor.

        Returns:
          An ordered list of events that have happened since the event
          cursor, from least recent to most recent.
        """
        ...
