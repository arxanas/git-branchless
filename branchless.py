#!/usr/bin/env python3
import argparse
import asyncio
import collections
import logging
import os
import pygit2
import re
import time
import sys

from dataclasses import dataclass
from typing import cast, List, Optional, Dict


def get_repo() -> pygit2.Repository:
    repo_path: Optional[str] = pygit2.discover_repository(os.getcwd())
    return pygit2.Repository(repo_path)


@dataclass(frozen=True, eq=True)
class RefLogAction:
    action: str
    action_message: Optional[str]

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


class RefLogReplayer:
    def __init__(self, head_ref: pygit2.Reference) -> None:
        head_oid: pygit2.Oid = head_ref.resolve().target
        self._head_oid: pygit2.Oid = head_oid
        self._current_oid: pygit2.Oid = head_oid
        self.commit_history: Dict[
            pygit2.Oid, List[pygit2.RefLogEntry]
        ] = collections.defaultdict(list)

        # Ensure that the head commit is marked as reachable to begin with.
        self.commit_history[head_oid] = []

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
            self.commit_history[entry.oid_new].append(entry)
        elif (
            action.action
            in [
                "rebase finished",
                "rebase (finish)",
                "rebase -i (finish)",
                "rebase",  # XXX
                "pull",
            ]
            or action.action.startswith("merge ")
            or action.action.startswith("pull --rebase")
        ):
            self.mark_unreachable(entry.oid_old)
            self.commit_history[entry.oid_new].append(entry)
        else:
            logging.warning(
                f"Reflog entry action type '{action.action}' ignored: {entry.oid_old} -> {entry.oid_new}: {entry.message}'"
            )

    def is_head(self, oid: pygit2.Oid) -> bool:
        return cast(bool, oid == self._head_oid)

    def mark_unreachable(self, oid: pygit2.Oid) -> None:
        if self.is_head(oid):
            # HEAD is always reachable.
            return

        try:
            # Mark the commit as unreachable, if present.
            del self.commit_history[oid]
        except KeyError:
            pass

    def reachable_commits(self) -> List[pygit2.Oid]:
        return list(self.commit_history.keys())


def first_line(message: str) -> str:
    return message.split("\n", 1)[0]


def is_commit_old(commit: pygit2.Commit, now: int) -> bool:
    # String like "-0430"
    offset_str = str(commit.commit_time_offset).zfill(5)
    offset_sign = -1 if offset_str[0] == "-" else 1
    offset_hours = int(offset_str[1:3])
    offset_minutes = int(offset_str[3:5])

    offset_seconds = offset_sign * ((offset_hours * 60 * 60) + (offset_minutes * 60))
    commit_timestamp: int = commit.commit_time
    commit_timestamp += offset_seconds
    max_age = 14 * 24 * 60 * 60  # 2 weeks
    return commit_timestamp < (now - max_age)


def oid_to_str(oid: pygit2.Oid) -> str:
    return f"{oid!s:8.8}"


def smartlog(*, show_old_commits: bool) -> None:
    repo = get_repo()
    # We don't use `repo.head`, because that resolves the HEAD reference
    # (e.g. into refs/head/master). We want the actual ref-log of HEAD, not
    # the reference it points to.
    head_ref = repo.references["HEAD"]
    replayer = RefLogReplayer(head_ref)
    for entry in head_ref.log():
        replayer.process(entry)

    master = repo.branches["master"].target

    now = int(time.time())
    num_old_commits = 0
    for oid, history in replayer.commit_history.items():
        commit: pygit2.Commit = repo[oid]
        (num_ahead, num_behind) = repo.ahead_behind(oid, master)
        is_master_commit = num_ahead == 0
        if is_master_commit and not replayer.is_head(oid):
            # Do not display.
            pass
        elif is_commit_old(commit, now=now):
            num_old_commits += 1
        else:
            print(f"{oid_to_str(oid)} {first_line(commit.message)}")
    if num_old_commits > 0:
        print(f"({num_old_commits} old commits hidden, use --show-old to show)")


def main(argv: List[str]) -> None:
    logging.basicConfig(level=logging.DEBUG)

    parser = argparse.ArgumentParser(prog="branchless")
    subparsers = parser.add_subparsers(
        dest="subcommand",
    )
    smartlog_parser = subparsers.add_parser(
        "smartlog",
        aliases=["sl"],
        help="Display a nice graph of commits you've recently worked on.",
    )
    smartlog_parser.add_argument(
        "--show-old", action="store_true", help="Show old commits (hidden by default)."
    )

    hide_parser = subparsers.add_parser("hide", help="hide a commit from the smartlog")
    args = parser.parse_args(argv)

    if args.subcommand in ["smartlog", "sl"]:
        smartlog(show_old_commits=args.show_old)
    elif args.subcommand == "hide":
        raise NotImplementedError()
    else:
        parser.print_usage()
        sys.exit(1)


if __name__ == "__main__":
    main(sys.argv[1:])