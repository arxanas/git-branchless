#!/usr/bin/env python3
import argparse
import asyncio
import collections
import logging
import os
import re
import string
import sys
import time
from dataclasses import dataclass
from typing import Any, Dict, Iterator, List, Mapping, Optional, TextIO, Union, cast

import pygit2


def get_repo() -> pygit2.Repository:
    repo_path: Optional[str] = pygit2.discover_repository(os.getcwd())
    return pygit2.Repository(repo_path)


class Formatter(string.Formatter):
    def format_field(self, value: Any, format_spec: str) -> str:
        if format_spec == "oid":
            assert isinstance(value, pygit2.Oid)
            return f"{value!s:8.8}"
        elif format_spec == "commit":
            assert isinstance(value, pygit2.Commit)
            message = cast(str, value.message)
            first_line = message.split("\n", 1)[0]
            return first_line
        else:
            raise ValueError(f"Unknown format spec {format_spec}")


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


@dataclass(frozen=True, eq=True)
class MarkedUnreachable:
    ref_log_entry: pygit2.RefLogEntry


class RefLogReplayer:
    CommitHistory = List[Union[pygit2.RefLogEntry, MarkedUnreachable]]

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
            self._mark_unreachable(oid=entry.oid_old, entry=entry)
            self._commit_history[entry.oid_new].append(entry)
        else:
            logging.warning(
                f"Reflog entry action type '{action.action}' ignored: {entry.oid_old} -> {entry.oid_new}: {entry.message}'"
            )

    def is_head(self, oid: pygit2.Oid) -> bool:
        return cast(bool, oid == self._head_oid)

    def _is_reachable(self, oid: pygit2.Oid) -> bool:
        if self.is_head(oid):
            return True

        # Don't instantiate an empty list for this oid if it's not in the
        # history.
        history = self._commit_history.get(oid)
        if history is None:
            return False
        else:
            return not isinstance(history[-1], MarkedUnreachable)

    def _mark_unreachable(self, oid: pygit2.Oid, entry: pygit2.RefLogEntry) -> None:
        self._commit_history[oid].append(MarkedUnreachable(ref_log_entry=entry))

    def reachable_commits(self) -> Iterator[pygit2.Oid]:
        return (oid for oid in self._commit_history.keys() if self._is_reachable(oid))


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


def smartlog(*, out: TextIO, show_old_commits: bool) -> None:
    """Display a nice graph of commits you've recently worked on."""
    formatter = Formatter()
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
    for oid, history in reversed(replayer.commit_history.items()):
        commit: pygit2.Commit = repo[oid]
        (num_ahead, num_behind) = repo.ahead_behind(oid, master)
        is_master_commit = num_ahead == 0
        if is_master_commit and not replayer.is_head(oid):
            # Do not display.
            logging.debug(
                formatter.format(
                    "Commit {oid:oid} is a master commit and not HEAD, not showing",
                    oid=oid,
                )
            )
        elif is_commit_old(commit, now=now):
            num_old_commits += 1
            logging.debug(
                formatter.format("Commit {oid:oid} is too old to be displayed", oid=oid)
            )
        else:
            out.write(
                formatter.format("{oid:oid} {commit:commit}\n", oid=oid, commit=commit)
            )
    if num_old_commits > 0:
        out.write(
            formatter.format(
                "({num_old_commits} old commits hidden, use --show-old to show)\n",
                num_old_commits=num_old_commits,
            )
        )


def debug_ref_log_entry(*, out: TextIO, hash: str) -> None:
    """(debug) Show all entries in the ref-log that affected a given commit."""
    formatter = Formatter()
    repo = get_repo()
    commit = repo[hash]
    commit_oid = commit.oid

    head_ref = repo.references["HEAD"]
    replayer = RefLogReplayer(head_ref)
    out.write(f"Ref-log entries that involved {commit_oid!s}\n")
    for entry in head_ref.log():
        replayer.process(entry)
        if commit_oid in [entry.oid_old, entry.oid_new]:
            out.write(
                formatter.format(
                    "{entry.oid_old:oid} -> {entry.oid_new:oid} {entry.message}: {commit:commit}\n",
                    entry=entry,
                    commit=commit,
                )
            )

    out.write(f"Reachable commit history for {commit_oid!s}\n")
    history = replayer.commit_history.get(commit_oid)
    if history is None:
        out.write("<none>\n")
    else:
        for entry in history:
            if isinstance(entry, MarkedUnreachable):
                entry = entry.ref_log_entry
                out.write(
                    formatter.format(
                        "DELETED {entry.oid_old:oid} -> {entry.oid_new:oid} {entry.message}: {commit:commit}\n",
                        entry=entry,
                        commit=commit,
                    )
                )
            else:
                assert isinstance(entry, pygit2.RefLogEntry)
                out.write(
                    formatter.format(
                        "{entry.oid_old:oid} -> {entry.oid_new:oid} {entry.message}: {commit:commit}\n",
                        entry=entry,
                        commit=commit,
                    )
                )


def main(argv: List[str], *, out: TextIO = sys.stdout) -> None:
    logging.basicConfig(level=logging.DEBUG)

    parser = argparse.ArgumentParser(prog="branchless")
    subparsers = parser.add_subparsers(
        dest="subcommand",
    )
    smartlog_parser = subparsers.add_parser(
        "smartlog",
        aliases=["sl"],
        help=smartlog.__doc__,
    )
    smartlog_parser.add_argument(
        "--show-old", action="store_true", help="Show old commits (hidden by default)."
    )
    hide_parser = subparsers.add_parser("hide", help="hide a commit from the smartlog")
    debug_ref_log_entry_parser = subparsers.add_parser(
        "debug-ref-log-entry", help=debug_ref_log_entry.__doc__
    )
    debug_ref_log_entry_parser.add_argument("hash", type=str)
    args = parser.parse_args(argv)

    if args.subcommand in ["smartlog", "sl"]:
        smartlog(out=out, show_old_commits=args.show_old)
    elif args.subcommand == "debug-ref-log-entry":
        debug_ref_log_entry(out=out, hash=args.hash)
    elif args.subcommand == "hide":
        raise NotImplementedError()
    else:
        parser.print_usage()
        sys.exit(1)


if __name__ == "__main__":
    main(sys.argv[1:])
