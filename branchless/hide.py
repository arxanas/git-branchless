"""Handle hiding commits explicitly."""
import time
from typing import Callable, Iterator, List, Set, TextIO

import pygit2

from . import get_repo
from .db import make_db_for_repo
from .eventlog import EventLogDb, EventReplayer, HideEvent, OidStr, UnhideEvent
from .formatting import make_glyphs
from .graph import Node, get_master_oid, make_graph
from .mergebase import MergeBaseDb
from .metadata import CommitMessageProvider, CommitOidProvider, render_commit_metadata


class CommitNotFoundError(Exception):
    def __init__(self, hash: str) -> None:
        self._hash = hash

    def __str__(self) -> str:
        return f"Commit not found: {self._hash}\n"


def _process_hashes(
    out: TextIO,
    repo: pygit2.Repository,
    event_replayer: EventReplayer,
    hashes: List[str],
) -> List[OidStr]:
    oids = []
    for hash in hashes:
        try:
            oid = repo.revparse_single(hash).oid
        except KeyError as e:
            raise CommitNotFoundError(hash) from e
        oids.append(oid.hex)
    return oids


def _recurse_on_oids(
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    event_replayer: EventReplayer,
    oids: List[OidStr],
    condition: Callable[[Node], bool],
) -> List[OidStr]:
    master_oid = get_master_oid(repo)
    (_head_oid, graph) = make_graph(
        repo=repo,
        merge_base_db=merge_base_db,
        event_replayer=event_replayer,
        master_oid=master_oid,
        hide_commits=False,
    )

    def helper(oid: OidStr) -> Iterator[OidStr]:
        node = graph[oid]
        if condition(node):
            yield oid

        for child_oid in node.children:
            yield from helper(child_oid)

    # Maintain ordering, since it's likely to be meaningful.
    result: List[OidStr] = list()
    seen_oids: Set[OidStr] = set()
    for oid in oids:
        for oid_to_add in helper(oid):
            if oid_to_add not in seen_oids:
                seen_oids.add(oid_to_add)
                result.append(oid_to_add)
    return result


def hide(*, out: TextIO, hashes: List[str], recursive: bool) -> int:
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
    db = make_db_for_repo(repo=repo)
    event_log_db = EventLogDb(db)
    event_replayer = EventReplayer.from_event_log_db(event_log_db)
    merge_base_db = MergeBaseDb(db)

    try:
        oids = _process_hashes(
            out=out, repo=repo, event_replayer=event_replayer, hashes=hashes
        )
    except CommitNotFoundError as e:
        out.write(str(e))
        return 1

    if recursive:
        oids = _recurse_on_oids(
            repo=repo,
            merge_base_db=merge_base_db,
            event_replayer=event_replayer,
            oids=oids,
            condition=lambda node: node.is_visible,
        )
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
        if event_replayer.get_commit_visibility(commit.oid.hex) == "hidden":
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


def unhide(*, out: TextIO, hashes: List[str], recursive: bool) -> int:
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
    db = make_db_for_repo(repo=repo)
    event_log_db = EventLogDb(db)
    event_replayer = EventReplayer.from_event_log_db(event_log_db)
    merge_base_db = MergeBaseDb(db)

    try:
        oids = _process_hashes(
            repo=repo, out=out, hashes=hashes, event_replayer=event_replayer
        )
    except CommitNotFoundError as e:
        out.write(str(e))

        return 1
    if recursive:
        oids = _recurse_on_oids(
            repo=repo,
            merge_base_db=merge_base_db,
            event_replayer=event_replayer,
            oids=oids,
            condition=lambda node: not node.is_visible,
        )
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
        if event_replayer.get_commit_visibility(commit.oid.hex) == "visible":
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
