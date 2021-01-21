"""Handle "restacking" commits which were abandoned due to rewrites.

The branchless workflow promotes checking out to arbitrary commits and
operating on them directly. However, if you e.g. amend a commit in-place, its
descendants will be abandoned.

For example, suppose we have this graph:

```
:
O abc000 master
|
@ abc001 Commit 1
|
o abc002 Commit 2
|
o abc003 Commit 3
```

And then we amend the current commit ("Commit 1"). The descendant commits
"Commit 2" and "Commit 3" will be abandoned:

```
:
O abc000 master
|\\
| x abc001 Commit 1
| |
| o abc002 Commit 2
| |
| o abc003 Commit 3
|
o def001 Commit 1 amended
```

The "restack" operation finds abandoned commits and rebases them to where
they should belong, resulting in a commit graph like this (note that the
hidden commits would not ordinarily be displayed; we show them only for the
sake of example here):

```
:
O abc000 master
|\\
| x abc001 Commit 1
| |
| x abc002 Commit 2
| |
| x abc003 Commit 3
|
o def001 Commit 1 amended
|
o def002 Commit 2
|
o def003 Commit 3
```
"""
from typing import List, Optional, TextIO, Tuple

from . import OidStr
from .eventlog import EventReplayer, RewriteEvent
from .graph import CommitGraph
from .rust import py_restack
from .smartlog import smartlog


def _find_rewrite_target(
    graph: CommitGraph, event_replayer: EventReplayer, oid: OidStr
) -> Optional[OidStr]:
    event = event_replayer.get_commit_latest_event(oid)
    if not isinstance(event, RewriteEvent):
        return None

    if event.old_commit_oid == oid and event.new_commit_oid != oid:
        new_oid = event.new_commit_oid
        possible_newer_oid = _find_rewrite_target(
            graph=graph, event_replayer=event_replayer, oid=new_oid
        )
        if possible_newer_oid is not None:
            return possible_newer_oid
        else:
            return new_oid
    else:
        return None


def find_abandoned_children(
    graph: CommitGraph, event_replayer: EventReplayer, oid: OidStr
) -> Optional[Tuple[OidStr, List[OidStr]]]:
    rewritten_oid = _find_rewrite_target(
        graph=graph, event_replayer=event_replayer, oid=oid
    )
    if rewritten_oid is None:
        return None

    # Adjacent main branch commits are not linked in the commit graph, but
    # if the user rewrote a main branch commit, then we may need to restack
    # subsequent main branch commits. Find the real set of children commits
    # so that we can do this.
    real_children_oids = set(graph[oid].children)
    for possible_child_oid in graph.keys():
        if possible_child_oid in real_children_oids:
            continue
        possible_child_node = graph[possible_child_oid]
        if any(
            parent_oid.hex == oid
            for parent_oid in possible_child_node.commit.parent_ids
        ):
            real_children_oids.add(possible_child_oid)

    return (
        rewritten_oid,
        [child_oid for child_oid in real_children_oids if graph[child_oid].is_visible],
    )


def restack(
    *, out: TextIO, err: TextIO, git_executable: str, preserve_timestamps: bool
) -> int:
    """Restack all abandoned commits.

    Args:
      out: The output stream to write to.
      err: The error stream to write to.
      git_executable: The path to the `git` executable on disk.
      preserve_timestamps: Whether or not to use the original commit time for
        rebased commits, rather than the current time.

    Returns:
      Exit code (0 denotes successful exit).
    """
    result = py_restack(
        out=out,
        err=err,
        git_executable=git_executable,
        preserve_timestamps=preserve_timestamps,
    )
    if result != 0:
        return result

    # TODO: `py_restack` should also display smartlog.
    return smartlog(out=out)
