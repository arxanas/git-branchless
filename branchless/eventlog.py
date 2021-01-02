"""Process our event log.

We use Git hooks to record the actions that the user takes over time, and put
them in persistent storage. Later, we play back the actions in order to
determine what actions the user took on the repository, and which commits
they're still working on.
"""
import collections
import enum
from dataclasses import dataclass
from typing import TYPE_CHECKING, Dict, List, Optional, Set, Tuple, Union

import pygit2
from typing_extensions import Literal

from . import OidStr, get_main_branch_name, get_main_branch_oid
from .rust import PyCommitEvent as CommitEvent
from .rust import PyEventLogDb as EventLogDb
from .rust import PyHideEvent as HideEvent
from .rust import PyRefUpdateEvent as RefUpdateEvent
from .rust import PyRewriteEvent as RewriteEvent
from .rust import PyUnhideEvent as UnhideEvent

if TYPE_CHECKING:
    from .rust import Event as Event
else:

    class Event:
        pass


NULL_OID = "0" * 40
"""Denotes the lack of an OID.

This could happen e.g. when creating or deleting a reference.
"""


class _EventClassification(enum.Enum):
    SHOW = enum.auto()
    HIDE = enum.auto()


@dataclass
class _EventInfo:
    id: int
    event: Event
    event_classification: _EventClassification


def is_gc_ref(ref_name: str) -> bool:
    """Determine whether a given reference is used to keep a commit alive.

    Args:
      ref_name: The name of the reference.

    Returns:
      Whether or not the given reference is used internally to keep the
      commit alive, so that it's not collected by Git's garbage collection
      mechanism.
    """
    return ref_name.startswith("refs/branchless/")


def should_ignore_ref_updates(ref_name: str) -> bool:
    return ref_name in ["ORIG_HEAD", "CHERRY_PICK_HEAD"] or is_gc_ref(ref_name)


class EventReplayer:
    """Processes events in order and determine the repo's visible commits."""

    def __init__(self) -> None:
        # Events are numbered starting from zero.
        self._id_counter = 0

        # Events up to this number (exclusive) are available to the caller.
        self._cursor_event_id = 0

        self._events: List[Event] = []
        self._commit_history: Dict[str, List[_EventInfo]] = collections.defaultdict(
            list
        )

        self._ref_logs_cached: Dict[str, List[RefUpdateEvent]] = {}

    @classmethod
    def from_event_log_db(cls, event_log_db: EventLogDb) -> "EventReplayer":
        """Construct the replayer from all the events in the database.

        Args:
          event_log_db: The database to query events from.

        Returns:
          The constructed replayer.
        """
        result = cls()
        for event in event_log_db.get_events():
            result.process_event(event)
        return result

    def process_event(self, event: Event) -> None:
        """Process the given event.

        This also sets the event cursor to point to immediately after the
        event that was just processed.

        Args:
          event: The next event to process. Events should be passed to the
          replayer in order from oldest to newest.
        """
        if isinstance(event, RefUpdateEvent) and (
            should_ignore_ref_updates(event.ref_name)
            or (event.old_ref is None and event.new_ref is None)
        ):
            # Non-meaningful event. Drop it.
            return

        id = self._id_counter
        self._id_counter += 1
        self._cursor_event_id = self._id_counter
        self._events.append(event)

        if isinstance(event, RewriteEvent):
            self._commit_history[event.old_commit_oid].append(
                _EventInfo(
                    id=id,
                    event=event,
                    event_classification=_EventClassification.HIDE,
                )
            )
            self._commit_history[event.new_commit_oid].append(
                _EventInfo(
                    id=id,
                    event=event,
                    event_classification=_EventClassification.SHOW,
                )
            )
        elif isinstance(event, RefUpdateEvent):
            # Currently, we don't process this.
            pass
        elif isinstance(event, CommitEvent):
            self._commit_history[event.commit_oid].append(
                _EventInfo(
                    id=id,
                    event=event,
                    event_classification=_EventClassification.SHOW,
                )
            )
        elif isinstance(event, HideEvent):
            self._commit_history[event.commit_oid].append(
                _EventInfo(
                    id=id,
                    event=event,
                    event_classification=_EventClassification.HIDE,
                )
            )
        elif isinstance(event, UnhideEvent):
            self._commit_history[event.commit_oid].append(
                _EventInfo(
                    id=id,
                    event=event,
                    event_classification=_EventClassification.SHOW,
                )
            )
        else:  # pragma: no cover
            raise TypeError(f"Unhandled event: {event}")

    def _get_commit_history(self, oid: OidStr) -> List[_EventInfo]:
        if oid not in self._commit_history:
            return []
        return [
            event_info
            for event_info in self._commit_history[oid]
            if event_info.id < self._cursor_event_id
        ]

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
        history = self._get_commit_history(oid)
        if not history:
            return None

        event_info = history[-1]
        if event_info.event_classification is _EventClassification.SHOW:
            return "visible"
        else:
            return "hidden"

    def get_commit_latest_event(self, oid: OidStr) -> Optional[Event]:
        """Get the latest event affecting a given commit.

        Args:
          oid: The OID of the commit to check.

        Returns:
          The most recent event that affected that commit. If this commit was
          not observed by the replayer, returns `None`.
        """
        history = self._get_commit_history(oid)
        if not history:
            return None

        event_info = history[-1]
        return event_info.event

    def get_active_oids(self) -> Set[str]:
        """Get the OIDs which have activity according to the repository history.

        Returns:
          The set of OIDs referring to commits which are thought to be active
          due to user action.
        """
        return set(
            oid
            for oid, history in self._commit_history.items()
            if any(event.id < self._cursor_event_id for event in history)
        )

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
        self._cursor_event_id = event_id
        if self._cursor_event_id < 0:
            self._cursor_event_id = 0
        elif self._cursor_event_id > len(self._events):
            self._cursor_event_id = len(self._events)

    def advance_cursor(self, num_events: int) -> None:
        """Advance the event cursor by the specified number of events.

        Args:
          num_events: The number of events to advance by. Can be positive,
            zero, or negative. If out of bounds, the cursor is set to the
            first or last valid position, as appropriate.
        """
        self.set_cursor(self._cursor_event_id + num_events)

    def get_cursor_head_oid(self) -> Optional[OidStr]:
        """Get the OID of `HEAD` at the cursor's point in time.

        Returns:
          The OID pointed to by `HEAD` at that time, or `None` if `HEAD` was
          never observed.
        """
        for i in range(self._cursor_event_id - 1, -1, -1):
            event = self._events[i]
            if isinstance(event, RefUpdateEvent) and event.ref_name == "HEAD":
                return event.new_ref

            # Not strictly necessary, but helps to compensate in case the user
            # is not running Git v2.29 or above, and therefore don't have the
            # corresponding `RefUpdateEvent`.
            elif isinstance(event, CommitEvent):
                return event.commit_oid
        return None

    def _get_cursor_branch_oid(
        self, repo: pygit2.Repository, branch_name: str
    ) -> Optional[OidStr]:
        if not (0 <= self._cursor_event_id - 1 < len(self._events)):
            return None

        ref_name = f"refs/heads/{branch_name}"
        for event in self._events[self._cursor_event_id - 1 :: -1]:
            if isinstance(event, RefUpdateEvent) and event.ref_name == ref_name:
                return event.new_ref
        return None

    def get_cursor_main_branch_oid(self, repo: pygit2.Repository) -> pygit2.Oid:
        main_branch_name = get_main_branch_name(repo)
        main_branch_oid = self._get_cursor_branch_oid(
            repo=repo, branch_name=main_branch_name
        )

        if main_branch_oid is None:
            # Assume the main branch just hasn't been observed moving yet, so
            # its value at the current time is fine to use.
            return get_main_branch_oid(repo)
        else:
            return repo[main_branch_oid].oid

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
        ref_name_to_oid: Dict[str, OidStr] = {}
        for event in self._events[: self._cursor_event_id]:
            if isinstance(event, RefUpdateEvent):
                if event.new_ref is not None:
                    ref_name_to_oid[event.ref_name] = event.new_ref
                elif event.ref_name in ref_name_to_oid:
                    del ref_name_to_oid[event.ref_name]

        result: Dict[OidStr, Set[str]] = {}
        for ref_name, ref_oid in ref_name_to_oid.items():
            if not ref_name.startswith("refs/heads/"):
                continue
            branch_name = ref_name[len("refs/heads/") :]
            if ref_oid not in result:
                result[ref_oid] = set()
            result[ref_oid].add(branch_name)

        main_branch_name = get_main_branch_name(repo)
        main_branch_oid = self.get_cursor_main_branch_oid(repo).hex
        if main_branch_oid not in result:
            result[main_branch_oid] = set()
        result[main_branch_oid].add(main_branch_name)

        return result

    def get_event_before_cursor(self) -> Optional[Tuple[int, Event]]:
        """Get the event immediately before the cursor.

        Returns:
          A tuple of event ID and the event that most recently happened. If
          no event was before the event cursor, returns `None` instead.
        """
        if self._cursor_event_id == 0:
            return None
        return (self._cursor_event_id, self._events[self._cursor_event_id - 1])

    def get_events_since_cursor(self) -> List[Event]:
        """Get all the events that have happened since the event cursor.

        Returns:
          An ordered list of events that have happened since the event
          cursor, from least recent to most recent.
        """
        return self._events[self._cursor_event_id :]
