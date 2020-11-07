import collections
import enum
import logging
import re
from dataclasses import dataclass
from typing import Dict, List, Mapping, Optional, Sequence, Tuple, Union

import pygit2

import branchless.hide


@dataclass(frozen=True, eq=True)
class RefLogAction:
    """Wrapper around an action taken in a ref-log.

    We scrape these entries to attempt to determine what happened in the past
    to the HEAD ref.
    """

    action_type: str
    """An action type, like "checkout"."""

    action_message: Optional[str]
    """An additional piece of description for the action, like "moving from X to Y".

    Most, but not every, ref-log action has an additional piece of
    description attached to it. An example of a ref-log action which doesn't
    would be "initial pull".
    """

    oid_old: pygit2.Oid
    """The OID of the reference before the action took place (for debugging)."""

    oid_new: pygit2.Oid
    """The OID of the reference after the action took place (for debugging)."""

    REF_LOG_LINE_RE = re.compile(
        r"""
        ^
        (?P<action_type>[^:]+)
        (
            :[ ]
            (?P<action_message>.+)
        )?
        $
        """,
        re.VERBOSE,
    )

    @classmethod
    def from_entry(cls, entry: pygit2.RefLogEntry) -> "RefLogAction":
        message = entry.message
        match = cls.REF_LOG_LINE_RE.match(message)
        assert match is not None, f"Failed to parse ref-log message: {message}"
        action_type = match.group("action_type")
        action_message = match.group("action_message")

        return cls(
            action_type=action_type,
            action_message=action_message,
            oid_old=entry.oid_old,
            oid_new=entry.oid_new,
        )


@dataclass(frozen=True, eq=True)
class MarkedHidden:
    """Wrapper class to mark when an action marked a commit as "hidden".

    This detects actions that "hid" the given commit, usually due to
    replacing with it a new one. For example, rebasing a commit is
    essentially implemented by replaying the commit on top of a different
    base commit than it was originally applied to. There's no inherent
    relationship between the old and the new commit in the eyes of Git,
    except that if you had a branch checked out, it no longer points to the
    old version of the commit.

    In a branchless workflow, we can't use the presence or absence of a
    branch to determine if the user was still using a given commit. Instead,
    we read the ref-log to determine if there was an action which logically
    hid the commit.
    """

    action: RefLogAction


class ClassifiedActionType(enum.Enum):
    BRANCH = enum.auto()
    CHECKOUT = enum.auto()
    COMMIT = enum.auto()
    HIDE = enum.auto()
    INIT = enum.auto()
    MERGE = enum.auto()
    REWRITE = enum.auto()
    UNKNOWN = enum.auto()

    @classmethod
    def classify(
        cls, entry: pygit2.RefLogEntry, action_type: str
    ) -> "ClassifiedActionType":
        if action_type in [
            # Branching (may or may not be referring to the
            # currently-checked-out branch).
            "branch",
            "Branch",
        ]:
            return cls.BRANCH
        elif action_type in [
            "commit",
            "commit (initial)",
            "rebase (pick)",
            "rebase (fixup)",
            "rebase (continue)",
            "rebase (squash)",
            "rebase -i (pick)",
            "rebase -i (fixup)",
            "rebase finished",
            "rebase (finish)",
            "rebase -i (finish)",
            "rebase",
            "pull",
        ]:
            return cls.COMMIT
        elif action_type in [
            "reset",
            "checkout",
            branchless.hide.CHECKOUT_REF_LOG_COMMAND,
        ]:
            return cls.CHECKOUT
        elif action_type in [
            "initial pull",
        ]:
            return cls.INIT
        elif action_type in [branchless.hide.HIDE_REF_LOG_COMMAND]:
            return cls.HIDE
        elif (
            action_type
            in [
                "rebase (start)",
                "rebase -i (start)",
                "commit (amend)",
            ]
            or action_type.startswith("pull --rebase")
        ):
            return cls.REWRITE
        elif action_type.startswith("merge "):
            return cls.MERGE
        else:
            logging.warning(
                f"Reflog entry action type '{action_type}' ignored: {entry.oid_old} -> {entry.oid_new}: {entry.message}'"
            )
            return cls.UNKNOWN


class RefLogReplayer:
    """Replay ref-log entries to determine which commits are visible."""

    CommitHistory = List[Tuple[int, Union[RefLogAction, MarkedHidden]]]

    def __init__(self, head_oid: pygit2.Oid) -> None:
        self._head_oid: pygit2.Oid = head_oid
        self._current_oid: pygit2.Oid = head_oid
        self._timestamp = 0

        # Invariant: if present, the value is always a non-empty list.
        self._commit_history: Dict[
            pygit2.Oid, RefLogReplayer.CommitHistory
        ] = collections.defaultdict(list)

    @property
    def commit_history(self) -> Mapping[pygit2.Oid, CommitHistory]:
        """Mapping from OID to actions that affected that commit (for
        debugging).

        Note that the caller is expected to have supplied the ref log actions
        in *reverse* order, which means that each list of actions is ordered
        from most-recent to least-recent.
        """
        return self._commit_history

    def process(self, entry: pygit2.RefLogEntry) -> None:
        self._current_oid = entry.oid_new
        action = RefLogAction.from_entry(entry)

        # We want to hide the old OID *before* we register the action for the
        # new OID. For example, if an entry were to mark the current OID as
        # hidden, but stay on the same OID, then it should be marked visible
        # again. However, since we're processing ref log entries in reverse
        # order, we insert the `MarkedHidden` entry *after* we register the
        # action for the new OID.
        self.commit_history[entry.oid_new].append((self._timestamp, action))
        if self._does_action_hide_old_oid(entry=entry, action=action):
            self.commit_history[entry.oid_old].append(
                (self._timestamp, MarkedHidden(action=action))
            )

        self._timestamp += 1

    def _does_action_hide_old_oid(
        self, entry: pygit2.RefLogEntry, action: RefLogAction
    ) -> bool:
        action_class = ClassifiedActionType.classify(
            entry=entry, action_type=action.action_type
        )
        if (
            False
            or action_class is ClassifiedActionType.BRANCH
            or action_class is ClassifiedActionType.CHECKOUT
            or action_class is ClassifiedActionType.COMMIT
            or action_class is ClassifiedActionType.INIT
            or action_class is ClassifiedActionType.UNKNOWN
        ):
            return False
        elif (
            False
            or action_class is ClassifiedActionType.HIDE
            or action_class is ClassifiedActionType.MERGE
            or action_class is ClassifiedActionType.REWRITE
        ):
            return True

    def finish_processing(self) -> None:
        for v in self._commit_history.values():
            v.reverse()

    def is_head(self, oid: pygit2.Oid) -> bool:
        return oid == self._head_oid

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
            (_timestamp, action) = history[-1]
            return not isinstance(action, MarkedHidden)

    def get_visible_oids(self) -> Sequence[pygit2.Oid]:
        """Get all OIDs thought to be visible according to the ref-log."""
        return [oid for oid in self._commit_history.keys() if self._is_visible(oid)]
