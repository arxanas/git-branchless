"""Process the ref-log.

The "ref-log" is a recording of the history of "refs" for the repository. A
"ref" is a reference to either a commit (a "direct" reference) or another
reference (a "symbolic" reference). References include branches and the
special `HEAD` reference.

Each action that moves the ref from one commit to another is recorded in the
"ref log" for that ref. We're particularly interested in the HEAD ref,
because it tracks the user's navigation through the repo. (We could in
principle track the history of branches in the repo if we needed to.)

We scrape the ref-log entries and infer what happened to repo over time to
infer which commits the user is working on and which commits they aren't.
"""
import collections
import enum
import logging
import re
from dataclasses import dataclass
from typing import Dict, List, Mapping, Optional, Sequence, Tuple, Union

import pygit2


@dataclass(frozen=True, eq=True)
class _RefLogAction:
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
    def from_entry(cls, entry: pygit2.RefLogEntry) -> "_RefLogAction":
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
class _MarkedHidden:
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

    action: _RefLogAction


class _ClassifiedActionType(enum.Enum):
    BRANCH = enum.auto()
    CHECKOUT = enum.auto()
    COMMIT = enum.auto()
    INIT = enum.auto()
    MERGE = enum.auto()
    REWRITE = enum.auto()
    UNKNOWN = enum.auto()

    @classmethod
    def classify(
        cls, entry: pygit2.RefLogEntry, action_type: str
    ) -> "_ClassifiedActionType":
        if action_type in [
            # Branching (may or may not be referring to the
            # currently-checked-out branch).
            "branch",
            "Branch",
        ]:
            return cls.BRANCH
        elif action_type in [
            "commit (initial)",
            "commit",
            "pull",
            "rebase (continue)",
            "rebase (finish)",
            "rebase (pick)",
            "rebase -i (finish)",
            "rebase -i (pick)",
            "rebase finished",
            "rebase",
        ]:
            return cls.COMMIT
        elif action_type in [
            "reset",
            "checkout",
        ]:
            return cls.CHECKOUT
        elif action_type in [
            "clone",
            "initial pull",
        ]:
            return cls.INIT
        elif (
            action_type
            in [
                "cherry-pick",
                "commit (amend)",
                "rebase (abort)",
                "rebase (fixup)",
                "rebase (reword)",
                "rebase (squash)",
                "rebase (start)",
                "rebase -i (fixup)",
                "rebase -i (reword)",
                "rebase -i (squash)",
                "rebase -i (start)",
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

    CommitHistory = List[Tuple[int, Union[_RefLogAction, _MarkedHidden]]]
    """History of actions that occurred to each commit."""

    def __init__(self, head_oid: pygit2.Oid) -> None:
        """Constructor.

        Args:
          head_oid: The OID that HEAD currently points to.
        """
        self._head_oid: pygit2.Oid = head_oid
        self._current_oid: pygit2.Oid = head_oid
        self._timestamp = 0

        # Invariant: if present, the value is always a non-empty list.
        self._commit_history: Dict[
            pygit2.Oid, RefLogReplayer.CommitHistory
        ] = collections.defaultdict(list)

    @property
    def commit_history(
        self,
    ) -> Mapping[pygit2.Oid, CommitHistory]:
        """Mapping from OID to actions that affected that commit (for
        debugging).

        Note that the caller is expected to have supplied the ref log actions
        in *reverse* order, which means that each list of actions is ordered
        from most-recent to least-recent.

        Returns:
          The commit history observed by the replayer.
        """
        return self._commit_history

    def process(self, entry: pygit2.RefLogEntry) -> None:
        """Process the given ref-log entry.

        It's expected that this is called for each ref-log entry, starting
        from the most recent to the least recent.

        Args:
          entry: The ref-log entry.
        """
        self._current_oid = entry.oid_new
        action = _RefLogAction.from_entry(entry)

        # We want to hide the old OID *before* we register the action for the
        # new OID. For example, if an entry were to mark the current OID as
        # hidden, but stay on the same OID, then it should be marked visible
        # again. However, since we're processing ref log entries in reverse
        # order, we insert the `MarkedHidden` entry *after* we register the
        # action for the new OID.
        action_class = _ClassifiedActionType.classify(
            entry=entry, action_type=action.action_type
        )
        if self._does_action_vivify_new_oid(action_class=action_class):
            self.commit_history[entry.oid_new].append((self._timestamp, action))
        if self._does_action_hide_old_oid(action_class=action_class):
            self.commit_history[entry.oid_old].append(
                (self._timestamp, _MarkedHidden(action=action))
            )
        self._timestamp += 1

    def _does_action_vivify_new_oid(self, action_class: _ClassifiedActionType) -> bool:
        if (
            False
            or action_class is _ClassifiedActionType.COMMIT
            or action_class is _ClassifiedActionType.INIT
            or action_class is _ClassifiedActionType.MERGE
            or action_class is _ClassifiedActionType.REWRITE
        ):
            return True
        elif (
            False
            or action_class is _ClassifiedActionType.BRANCH
            or action_class is _ClassifiedActionType.CHECKOUT
            or action_class is _ClassifiedActionType.UNKNOWN
            or action_class is _ClassifiedActionType.HIDE
        ):
            return False

    def _does_action_hide_old_oid(self, action_class: _ClassifiedActionType) -> bool:
        if (
            False
            or action_class is _ClassifiedActionType.BRANCH
            or action_class is _ClassifiedActionType.CHECKOUT
            or action_class is _ClassifiedActionType.COMMIT
            or action_class is _ClassifiedActionType.INIT
            or action_class is _ClassifiedActionType.UNKNOWN
        ):
            return False
        elif (
            False
            or action_class is _ClassifiedActionType.MERGE
            or action_class is _ClassifiedActionType.REWRITE
        ):
            return True

    def finish_processing(self) -> None:
        """Must be called after all entries have been passed to `process`."""
        for v in self._commit_history.values():
            v.reverse()

    def is_head(self, oid: pygit2.Oid) -> bool:
        """Determine if the given OID is the HEAD OID.

        Args:
          oid: An OID.

        Returns:
          Whether or not the given OID the is OID pointed to by HEAD.
        """
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
            return not isinstance(action, _MarkedHidden)

    def get_visible_oids(self) -> Sequence[pygit2.Oid]:
        """Get all OIDs thought to be visible according to the ref-log.

        Returns:
          All visible OIDs. Order is not significant.
        """
        return [self._head_oid] + [
            oid for oid in self._commit_history.keys() if self._is_visible(oid)
        ]
