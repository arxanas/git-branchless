import collections
import logging
import re
from dataclasses import dataclass
from typing import Dict, Iterator, List, Optional, Union, cast

import pygit2


@dataclass(frozen=True, eq=True)
class RefLogAction:
    """Wrapper around an action taken in a ref-log.

    We scrape these entries to attempt to determine what happened in the past
    to the HEAD ref.
    """

    action: str
    """An action, like "checkout"."""

    action_message: Optional[str]
    """An additional piece of description for the action, like "moving from X to Y".

    Most, but not every, ref-log action has an additional piece of
    description attached to it. An example of a ref-log action which doesn't
    would be "initial pull".
    """

    REF_LOG_LINE_RE = re.compile(
        r"""
        ^
        (?P<action>[^:]+)
        (
            :[ ]
            (?P<action_message>.+)
        )?
        $
        """,
        re.VERBOSE,
    )

    @classmethod
    def parse_ref_log_message(cls, message: str) -> "RefLogAction":
        match = cls.REF_LOG_LINE_RE.match(message)
        assert match is not None, f"Failed to parse ref-log message: {message}"
        action = match.group("action")
        action_message = match.group("action_message")
        return cls(
            action=action,
            action_message=action_message,
        )


@dataclass(frozen=True, eq=True)
class MarkedHidden:
    """Wrapper for a ref-log entry which caused a commit to be marked as
    "hidden".

    Typically, this is for actions that "hid" the given commit, usually due
    to replacing with it a new one. For example, rebasing a commit is
    essentially implemented by replaying the commit on top of a different
    base commit than it was originally applied to. There's no inherent
    relationship between the old and the new commit in the eyes of Git,
    except that if you had a branch checked out, it no longer points to the
    old version of the commit.

    In a branchless workflow, we can't use the presence or absence of a
    branch to determine if the user was still using a given commit. Instead,
    we read the ref-log to determine if there was an action which logically
    hid the commit. When detected, we wrap those entries in this class.
    """

    ref_log_entry: pygit2.RefLogEntry


class RefLogReplayer:
    """Replay ref-log entries to determine which commits are visible."""

    CommitHistory = List[Union[pygit2.RefLogEntry, MarkedHidden]]

    def __init__(self, head_ref: pygit2.Reference) -> None:
        head_oid: pygit2.Oid = head_ref.resolve().target
        self._head_oid: pygit2.Oid = head_oid
        self._current_oid: pygit2.Oid = head_oid

        # Invariant: if present, the value is always a non-empty list.
        self._commit_history: Dict[
            pygit2.Oid, RefLogReplayer.CommitHistory
        ] = collections.defaultdict(list)

    @property
    def commit_history(self) -> Dict[pygit2.Oid, CommitHistory]:
        return self._commit_history

    def process(self, entry: pygit2.RefLogEntry) -> None:
        self._current_oid = entry.oid_new

        action = RefLogAction.parse_ref_log_message(entry.message)
        if action.action in [
            # Branching (may or may not be referring to the
            # currently-checked-out branch).
            "branch",
            "Branch",
            # Checking out to commit/adjusting working copy.
            "initial pull",
            "reset",
            "checkout",
            "rebase (start)",
            "rebase -i (start)",
            # Committing.
            "commit",
            "commit (amend)",
            "commit (initial)",
            "rebase (pick)",
            "rebase -i (pick)",
            "rebase -i (fixup)",
        ]:
            self._commit_history[entry.oid_new].append(entry)
        elif (
            action.action
            in [
                "rebase finished",
                "rebase (finish)",
                "rebase -i (finish)",
                "rebase",
                "pull",
            ]
            or action.action.startswith("merge ")
            or action.action.startswith("pull --rebase")
        ):
            self._mark_hidden(oid=entry.oid_old, entry=entry)
            self._commit_history[entry.oid_new].append(entry)
        else:
            logging.warning(
                f"Reflog entry action type '{action.action}' ignored: {entry.oid_old} -> {entry.oid_new}: {entry.message}'"
            )

    def is_head(self, oid: pygit2.Oid) -> bool:
        return cast(bool, oid == self._head_oid)

    def _is_visible(self, oid: pygit2.Oid) -> bool:
        # HEAD is always visible, since is denotes the commit you're currently
        # working on.
        if self.is_head(oid):
            return True

        # Don't instantiate an empty list for this oid if it's not in the
        # history.
        history = self._commit_history.get(oid)
        if history is None:
            return False
        else:
            return not isinstance(history[-1], MarkedHidden)

    def _mark_hidden(self, oid: pygit2.Oid, entry: pygit2.RefLogEntry) -> None:
        self._commit_history[oid].append(MarkedHidden(ref_log_entry=entry))

    def get_visible_commits(self) -> Iterator[pygit2.Oid]:
        """Get all commits thought to be visible according to the ref-log."""
        return (oid for oid in self._commit_history.keys() if self._is_visible(oid))
