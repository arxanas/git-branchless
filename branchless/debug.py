from typing import TextIO

import pygit2

from . import Formatter, get_repo
from .reflog import MarkedHidden, RefLogReplayer


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
        for entry_or_hidden in history:
            if isinstance(entry_or_hidden, MarkedHidden):
                out.write(
                    formatter.format(
                        "DELETED {entry.oid_old:oid} -> {entry.oid_new:oid} {entry.message}: {commit:commit}\n",
                        entry=entry_or_hidden.ref_log_entry,
                        commit=commit,
                    )
                )
            else:
                assert isinstance(entry_or_hidden, pygit2.RefLogEntry)
                out.write(
                    formatter.format(
                        "{entry.oid_old:oid} -> {entry.oid_new:oid} {entry.message}: {commit:commit}\n",
                        entry=entry_or_hidden,
                        commit=commit,
                    )
                )
