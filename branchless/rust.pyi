import sqlite3
from dataclasses import dataclass
from typing import Optional, Sequence, Union

import pygit2

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
