"""Process our event log.

We use Git hooks to record the actions that the user takes over time, and put
them in persistent storage. Later, we play back the actions in order to
determine what actions the user took on the repository, and which commits
they're still working on.
"""
from typing import TYPE_CHECKING

from .rust import PyCommitEvent as CommitEvent
from .rust import PyEventLogDb as EventLogDb
from .rust import PyEventReplayer as EventReplayer
from .rust import PyHideEvent as HideEvent
from .rust import PyRefUpdateEvent as RefUpdateEvent
from .rust import PyRewriteEvent as RewriteEvent
from .rust import PyUnhideEvent as UnhideEvent
from .rust import is_gc_ref as is_gc_ref
from .rust import should_ignore_ref_updates as should_ignore_ref_updates

if TYPE_CHECKING:
    from .rust import Event as Event
else:

    class Event:
        pass


# Silence flake8.
_ = (
    CommitEvent,
    EventLogDb,
    EventReplayer,
    HideEvent,
    RefUpdateEvent,
    RewriteEvent,
    UnhideEvent,
    is_gc_ref,
    should_ignore_ref_updates,
)
