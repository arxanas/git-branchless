"""Handle hiding commits explicitly."""
import time
from typing import List, TextIO, Tuple

import pygit2

from . import get_repo
from .db import make_db_for_repo
from .eventlog import EventLogDb, EventReplayer, HideEvent, OidStr, UnhideEvent
from .formatting import make_glyphs
from .metadata import CommitMessageProvider, CommitOidProvider, render_commit_metadata


class CommitNotFoundError(Exception):
    def __init__(self, hash: str) -> None:
        self._hash = hash

    def __str__(self) -> str:
        return f"Commit not found: {self._hash}\n"


def _process_hashes(
    out: TextIO, repo: pygit2.Repository, hashes: List[str]
) -> Tuple[EventReplayer, EventLogDb, List[OidStr]]:
    db = make_db_for_repo(repo=repo)
    event_log_db = EventLogDb(db)
    event_replayer = EventReplayer.from_event_log_db(event_log_db)

    oids = []
    for hash in hashes:
        try:
            oid = repo.revparse_single(hash).oid
        except KeyError as e:
            raise CommitNotFoundError(hash) from e
        oids.append(oid.hex)
    return (event_replayer, event_log_db, oids)


def hide(*, out: TextIO, hashes: List[str]) -> int:
    """Hide the hashes provided on the command-line.

    Args:
      out: The output stream to write to.
      hashes: A list of commit hashes to hide. Revs will be resolved (you can
        provide an abbreviated commit hash or ref name).

    Returns:
      Exit code (0 denotes successful exit).
    """
    timestamp = time.time()
    repo = get_repo()
    try:
        (replayer, event_log_db, oids) = _process_hashes(
            out=out, repo=repo, hashes=hashes
        )
    except CommitNotFoundError as e:
        out.write(str(e))
        return 1
    events = [HideEvent(timestamp=timestamp, commit_oid=oid) for oid in oids]
    event_log_db.add_events(events)

    glyphs = make_glyphs(out=out)
    for event in events:
        commit = repo[event.commit_oid]
        hidden_commit_text = render_commit_metadata(
            glyphs=glyphs,
            commit=commit,
            commit_metadata_providers=[
                CommitOidProvider(glyphs=glyphs, use_color=True),
                CommitMessageProvider(),
            ],
        )
        out.write(f"Hid commit: {hidden_commit_text}\n")
        if replayer.get_commit_visibility(commit.oid.hex) == "hidden":
            out.write("(It was already hidden, so this operation had no effect.)\n")

        command_target_oid = render_commit_metadata(
            glyphs=glyphs,
            commit=commit,
            commit_metadata_providers=[
                CommitOidProvider(glyphs=glyphs, use_color=False),
            ],
        )
        out.write(f"To unhide this commit, run: git unhide {command_target_oid}\n")
    return 0


def unhide(*, out: TextIO, hashes: List[str]) -> int:
    """Unhide the hashes provided on the command-line.

    Args:
      out: The output stream to write to.
      hashes: A list of commit hashes to unhide. Revs will be resolved (you
        can provide an abbreviated commit hash or ref name).

    Returns:
      Exit code (0 denotes successful exit).
    """
    timestamp = time.time()
    repo = get_repo()
    try:
        (replayer, event_log_db, oids) = _process_hashes(
            repo=repo, out=out, hashes=hashes
        )
    except CommitNotFoundError as e:
        out.write(str(e))
        return 1
    events = [UnhideEvent(timestamp=timestamp, commit_oid=oid) for oid in oids]
    event_log_db.add_events(events)

    glyphs = make_glyphs(out=out)
    for event in events:
        commit = repo[event.commit_oid]
        unhidden_commit_text = render_commit_metadata(
            glyphs=glyphs,
            commit=commit,
            commit_metadata_providers=[
                CommitOidProvider(glyphs=glyphs, use_color=True),
                CommitMessageProvider(),
            ],
        )
        out.write(f"Unhid commit: {unhidden_commit_text}\n")
        if replayer.get_commit_visibility(commit.oid.hex) == "visible":
            out.write("(It was not hidden, so this operation had no effect.)\n")

        command_target_oid = render_commit_metadata(
            glyphs=glyphs,
            commit=commit,
            commit_metadata_providers=[
                CommitOidProvider(glyphs=glyphs, use_color=False),
            ],
        )
        out.write(f"To hide this commit, run: git hide {command_target_oid}\n")
    return 0
