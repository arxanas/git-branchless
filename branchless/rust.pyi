import sqlite3
from dataclasses import dataclass
from typing import Dict, List, Optional, Sequence, Set, TextIO, Tuple, Union

import pygit2
from typing_extensions import Literal

from . import OidStr

# mergebase.py

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

# eventlog.py

def is_gc_ref(ref_name: str) -> bool:
    """Determine whether a given reference is used to keep a commit alive.

    Args:
      ref_name: The name of the reference.

    Returns:
      Whether or not the given reference is used internally to keep the
      commit alive, so that it's not collected by Git's garbage collection
      mechanism.
    """
    ...

def should_ignore_ref_updates(ref_name: str) -> bool: ...
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

# graph.py
@dataclass
class PyNode:
    """Node contained in the smartlog commit graph."""

    commit: pygit2.Commit
    """The underlying commit object."""

    parent: Optional[OidStr]
    """The OID of the parent node in the smartlog commit graph.

    This is different from inspecting `commit.parents`, since the smartlog
    will hide most nodes from the commit graph, including parent nodes.
    """

    children: Set[OidStr]
    """The OIDs of the children nodes in the smartlog commit graph."""

    is_main: bool
    """Indicates that this is a commit to the main branch.

    These commits are considered to be immutable and should never leave the
    `main` state. However, this can still happen sometimes if the user's
    workflow is different than expected.
    """

    is_visible: bool
    """Indicates that this commit should be considered "visible".

    A visible commit is a commit that hasn't been checked into the main
    branch, but the user is actively working on. We may infer this from user
    behavior, e.g. they committed something recently, so they are now working
    on it.

    In contrast, a hidden commit is a commit that hasn't been checked into
    the main branch, and the user is no longer working on. We may infer this
    from user behavior, e.g. they have rebased a commit and no longer want to
    see the old version of that commit. The user can also manually hide
    commits.

    Occasionally, a main commit can be marked as hidden, such as if a commit
    in the main branch has been rewritten. We don't expect this to happen in
    the monorepo workflow, but it can happen in other workflows where you
    commit directly to the main branch and then later rewrite the commit.
    """

    event: Optional[Event]
    """The latest event to affect this commit.

    It's possible that no event affected this commit, and it was simply
    visible due to a reference pointing to it. In that case, this field is
    `None`.
    """

def py_find_path_to_merge_base(
    repo: pygit2.Repository,
    merge_base_db: PyMergeBaseDb,
    commit_oid: pygit2.Oid,
    target_oid: pygit2.Oid,
) -> Optional[List[pygit2.Commit]]:
    """Find a shortest path between the given commits.

    This is particularly important for multi-parent commits (i.e. merge
    commits). If we don't happen to traverse the correct parent, we may end
    up traversing a huge amount of commit history, with a significant
    performance hit.

    Args:
      repo: The Git repository.
      commit_oid: The OID of the commit to start at. We take parents of the
        provided commit until we end up at the target OID.
      target_oid: The OID of the commit to end at.

    Returns:
      A path of commits from `commit_oid` through parents to `target_oid`.
      The path includes `commit_oid` at the beginning and `target_oid` at the
      end. If there is no such path, returns `None`.
    """
    ...

PyCommitGraph = Dict[OidStr, PyNode]
"""Graph of commits that the user is working on."""

def py_make_graph(
    repo: pygit2.Repository,
    merge_base_db: PyMergeBaseDb,
    event_replayer: PyEventReplayer,
    head_oid: Optional[OidStr],
    main_branch_oid: pygit2.Oid,
    branch_oids: Set[OidStr],
    hide_commits: bool,
) -> PyCommitGraph:
    """Construct the smartlog graph for the repo.

    Args:
      repo: The Git repository.
      merge_base_db: The merge-base database.
      event_replayer: The event replayer.
      head_oid: The OID of the repository's `HEAD` reference.
      main_branch_oid: The OID of the main branch.
      branch_oids: The set of OIDs pointed to by branches.
      hide_commits: If set to `True`, then, after constructing the graph,
        remove nodes from it that appear to be hidden by user activity. This
        should be set to `True` for most display-related purposes.

    Returns:
      A tuple of the head OID and the commit graph.
    """
    ...

# init.py
def py_init(*, out: TextIO, git_executable: str) -> None: ...

# gc.py

def py_mark_commit_reachable(repo: pygit2.Repository, commit_oid: pygit2.Oid) -> None:
    """Mark a commit as reachable.

    Once marked as reachable, the commit won't be collected by Git's garbage
    collection mechanism until first garbage-collected by branchless itself.

    Args:
      repo: The Git repository.
      commit_oid: The commit OID to mark as reachable.
    """
    ...

def py_gc(*, out: TextIO) -> None:
    """Run branchless's garbage collection.

    Args:
      out: The output stream to write to.
    """
    ...

# restack.py

def py_find_abandoned_children(
    graph: PyCommitGraph, event_replayer: PyEventReplayer, oid: OidStr
) -> Optional[Tuple[OidStr, List[OidStr]]]: ...
def py_restack(*, out: TextIO, err: TextIO, git_executable: str) -> int:
    """Restack all abandoned commits.

    Args:
      out: The output stream to write to.
      err: The error stream to write to.
      git_executable: The path to the `git` executable on disk.

    Returns:
      Exit code (0 denotes successful exit).
    """
    ...

# hooks.py

def py_hook_post_rewrite(out: TextIO, rewrite_type: str) -> None:
    """Handle Git's post-rewrite hook.

    Args:
      out: Output stream to write to.
      rewrite_type: The type of rewrite. Currently one of "rebase" or
        "amend".
    """
    ...

def py_hook_post_checkout(
    out: TextIO, previous_head_ref: str, current_head_ref: str, is_branch_checkout: int
) -> None:
    """Handle Git's post-checkout hook.

    Args:
      out: Output stream to write to.
    """
    ...

def py_hook_post_commit(out: TextIO) -> None:
    """Handle Git's post-commit hook.

    Args:
      out: Output stream to write to.
    """
    ...

def py_hook_reference_transaction(out: TextIO, transaction_state: str) -> None:
    """Handle Git's reference-transaction hook.

    Args:
      out: Output stream to write to.
    """
    ...

# smartlog.py

def py_render_graph(
    out: TextIO,
    glyphs: object,
    repo: pygit2.Repository,
    merge_base_db: PyMergeBaseDb,
    graph: PyCommitGraph,
    head_oid: Optional[OidStr],
    commit_metadata_providers: List[object],
) -> None: ...
def py_smartlog(*, out: TextIO) -> int:
    """Display a nice graph of commits you've recently worked on.

    Args:
      out: The output stream to write to.

    Returns:
      Exit code (0 denotes successful exit).
    """
    ...
