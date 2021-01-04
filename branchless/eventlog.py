"""Process our event log.

We use Git hooks to record the actions that the user takes over time, and put
them in persistent storage. Later, we play back the actions in order to
determine what actions the user took on the repository, and which commits
they're still working on.
"""
from typing import TYPE_CHECKING

from .rust import (
    PyCommitEvent,
    PyEventLogDb,
    PyEventReplayer,
    PyHideEvent,
    PyRefUpdateEvent,
    PyRewriteEvent,
    PyUnhideEvent,
)

if TYPE_CHECKING:
    from .rust import Event as Event
else:

    class Event:
        pass


NULL_OID = "0" * 40
"""Denotes the lack of an OID.

This could happen e.g. when creating or deleting a reference.
"""


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


CommitEvent = PyCommitEvent
HideEvent = PyHideEvent
RefUpdateEvent = PyRefUpdateEvent
RewriteEvent = PyRewriteEvent
UnhideEvent = PyUnhideEvent

EventLogDb = PyEventLogDb
EventReplayer = PyEventReplayer
