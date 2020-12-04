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

import pygit2

from . import (
    OidStr,
    get_branch_oid_to_names,
    get_head_oid,
    get_main_branch_oid,
    get_repo,
    run_git,
)
from .db import make_db_for_repo
from .eventlog import EventLogDb, EventReplayer, RewriteEvent
from .graph import CommitGraph, make_graph
from .mergebase import MergeBaseDb
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


def _restack_commits(
    out: TextIO,
    err: TextIO,
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    event_log_db: EventLogDb,
    preserve_timestamps: bool,
) -> int:
    event_replayer = EventReplayer.from_event_log_db(event_log_db)
    head_oid = get_head_oid(repo)
    main_branch_oid = get_main_branch_oid(repo)
    branch_oid_to_names = get_branch_oid_to_names(repo)
    graph = make_graph(
        repo=repo,
        merge_base_db=merge_base_db,
        event_replayer=event_replayer,
        head_oid=head_oid.hex,
        main_branch_oid=main_branch_oid,
        branch_oids=set(branch_oid_to_names),
        hide_commits=True,
    )

    for original_oid in graph:
        abandoned_result = find_abandoned_children(
            graph=graph, event_replayer=event_replayer, oid=original_oid
        )
        if not abandoned_result:
            continue

        (rewritten_oid, abandoned_child_oids) = abandoned_result
        if not abandoned_child_oids:
            continue

        # Pick an arbitrary abandoned child. We'll rewrite it and then repeat,
        # and next time, it won't be considered abandoned because it's been
        # rewritten.
        abandoned_oid = abandoned_child_oids[0]

        additional_args = []
        if preserve_timestamps:
            additional_args = ["--committer-date-is-author-date"]
        result = run_git(
            out=out,
            err=err,
            args=["rebase", original_oid, abandoned_oid, "--onto", rewritten_oid]
            + additional_args,
        )
        if result != 0:
            out.write("branchless: resolve rebase, then run 'git restack' again\n")
            return result

        # Repeat until we reach a fixed-point.
        return _restack_commits(
            out=out,
            err=err,
            repo=repo,
            merge_base_db=merge_base_db,
            event_log_db=event_log_db,
            preserve_timestamps=preserve_timestamps,
        )

    out.write("branchless: no more abandoned commits to restack\n")
    return 0


def _restack_branches(
    out: TextIO,
    err: TextIO,
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    event_log_db: EventLogDb,
) -> int:
    event_replayer = EventReplayer.from_event_log_db(event_log_db)
    head_oid = get_head_oid(repo)
    main_branch_oid = get_main_branch_oid(repo)
    branch_oid_to_names = get_branch_oid_to_names(repo)
    graph = make_graph(
        repo=repo,
        merge_base_db=merge_base_db,
        event_replayer=event_replayer,
        head_oid=head_oid.hex,
        main_branch_oid=main_branch_oid,
        branch_oids=set(branch_oid_to_names),
        hide_commits=True,
    )

    for branch_name in repo.listall_branches(pygit2.GIT_BRANCH_LOCAL):
        branch = repo.branches[branch_name]
        if branch.target.hex not in graph:
            continue

        new_oid = _find_rewrite_target(
            graph=graph, event_replayer=event_replayer, oid=branch.target.hex
        )
        if new_oid is None:
            continue

        result = run_git(out=out, err=err, args=["branch", "-f", branch_name, new_oid])
        if result != 0:
            return result
        else:
            return _restack_branches(
                out=out,
                err=err,
                repo=repo,
                merge_base_db=merge_base_db,
                event_log_db=event_log_db,
            )

    out.write("branchless: no more abandoned branches to restack\n")
    return 0


def restack(out: TextIO, err: TextIO, preserve_timestamps: bool) -> int:
    """Restack all abandoned commits.

    Args:
      out: The output stream to write to.
      preserve_timestamps: Whether or not to use the original commit time for
        rebased commits, rather than the current time.

    Returns:
      Exit code (0 denotes successful exit).
    """
    repo = get_repo()
    db = make_db_for_repo(repo)
    merge_base_db = MergeBaseDb(db)
    event_log_db = EventLogDb(db)

    head_type = repo.references["HEAD"].type
    if head_type == pygit2.GIT_REF_SYMBOLIC:
        head_location = repo.head.shorthand
    else:
        assert head_type == pygit2.GIT_REF_OID
        head_location = repo.head.target.hex

    result = _restack_commits(
        out=out,
        err=err,
        repo=repo,
        merge_base_db=merge_base_db,
        event_log_db=event_log_db,
        preserve_timestamps=preserve_timestamps,
    )
    if result != 0:
        return result

    result = _restack_branches(
        out=out,
        err=err,
        repo=repo,
        merge_base_db=merge_base_db,
        event_log_db=event_log_db,
    )
    if result != 0:
        return result

    result = run_git(out=out, err=err, args=["checkout", head_location])
    if result != 0:
        return result

    return smartlog(out=out)
