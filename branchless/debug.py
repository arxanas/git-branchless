from typing import TextIO

import pygit2

from . import Formatter, get_repo
from .reflog import RefLogReplayer


def debug_ref_log_entry(*, out: TextIO, hash: str) -> None:
    """(debug) Show all entries in the ref-log that affected a given commit."""
    formatter = Formatter()
    repo = get_repo()
    commit = repo[hash]
    commit_oid = commit.oid

    head_ref = repo.references["HEAD"]
    head_oid = head_ref.resolve().target
    replayer = RefLogReplayer(head_oid)
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
        for (_timestamp, action) in history:
            if action.did_mark_hidden:
                out.write(
                    formatter.format(
                        "DELETED {action.oid_old:oid} -> {action.oid_new:oid} {action.message}: {commit:commit}\n",
                        action=action,
                        commit=commit,
                    )
                )
            else:
                assert isinstance(action, pygit2.RefLogEntry)
                out.write(
                    formatter.format(
                        "{action.oid_old:oid} -> {action.oid_new:oid} {action.message}: {commit:commit}\n",
                        action=action,
                        commit=commit,
                    )
                )
