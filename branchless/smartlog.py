import functools
import logging
import string
import time
from dataclasses import dataclass
from typing import Dict, Iterator, List, Optional, Sequence, Set, TextIO

import pygit2

from . import CommitStatus, Formatter, get_repo
from .reflog import RefLogReplayer


def is_commit_old(commit: pygit2.Commit, now: int) -> bool:
    """Determine if a commit has not been touched for a while (is "old").

    Such commits are visible, but by default, not shown by the smartlog.
    """
    max_age = 14 * 24 * 60 * 60  # 2 weeks
    return commit.commit_time < (now - max_age)


@dataclass
class DisplayedCommit:
    oid: pygit2.Oid
    parent: Optional[pygit2.Oid]
    children: Set[pygit2.Oid]
    status: CommitStatus


CommitGraph = Dict[pygit2.Oid, DisplayedCommit]


def walk_commit_parents(
    formatter: Formatter, repo: pygit2.Repository, commit_oid: pygit2.Oid
) -> Iterator[pygit2.Commit]:
    """Faster implementation of `repo.walk`.

    For some reason, `repo.walk` hangs for a while when trying to run on a
    large repo.
    """
    commit = repo[commit_oid]
    while True:
        yield commit
        parents = commit.parents
        if len(parents) == 0:
            break
        if len(parents) > 1:
            # TODO: we may choose the wrong one and never reach the intended merge-base.
            logging.debug(
                formatter.format(
                    "Multiple parents for commit {commit.oid:oid}, choosing one arbitrarily",
                    commit=commit,
                )
            )
        commit = parents[0]


def walk_from_visible_commits(
    formatter: Formatter,
    repo: pygit2.Repository,
    master_oid: pygit2.Oid,
    commit_oids: Sequence[pygit2.Oid],
) -> CommitGraph:
    """Find additional commits that should be displayed."""
    graph: CommitGraph = {}
    for commit_oid in commit_oids:
        merge_base_oid = repo.merge_base(commit_oid, master_oid)
        assert merge_base_oid is not None, formatter.format(
            "No merge-base found for commits {commit_oid:oid} and {master_oid:oid}",
            commit_oid=commit_oid,
            master_oid=master_oid,
        )

        previous_oid = None
        for current_commit in walk_commit_parents(
            formatter=formatter, repo=repo, commit_oid=commit_oid
        ):
            current_oid = current_commit.oid

            if current_oid not in graph:
                should_break = False
                graph[current_oid] = DisplayedCommit(
                    oid=current_oid, parent=None, children=set(), status="visible"
                )
            else:
                should_break = True

            if previous_oid is not None:
                graph[previous_oid].parent = current_oid
                graph[current_oid].children.add(previous_oid)

            if should_break:
                break

            if current_oid == merge_base_oid:
                graph[current_oid].status = "master"
                break

            previous_oid = current_oid
    return graph


def split_commit_graph_by_roots(
    formatter: string.Formatter, repo: pygit2.Repository, graph: CommitGraph
) -> List[pygit2.Oid]:
    """Split fully-independent subgraphs into multiple graphs.

    This is intended to if you have multiple lines of work rooted from
    different commits in master.

    Returns the list such that the topologically-earlier subgraphs are first
    in the list (i.e. those that would be rendered at the bottom of the
    smartlog).
    """
    root_commit_oids = [
        commit_oid
        for commit_oid, displayed_commit in graph.items()
        if displayed_commit.parent is None
    ]

    def compare(lhs: pygit2.Oid, rhs: pygit2.Oid) -> int:
        merge_base = repo.merge_base(lhs, rhs)
        if merge_base == lhs:
            # lhs was topologically first, so it should be sorted later in the list.
            return 1
        elif merge_base == rhs:
            return -1
        else:
            logging.warning(
                formatter.format(
                    "Root commits {lhs:oid} and {rhs:oid} were not orderable",
                    lhs=lhs,
                    rhs=rhs,
                )
            )
            return 0

    root_commit_oids.sort(key=functools.cmp_to_key(compare))
    return root_commit_oids


def walk_children(
    repo: pygit2.Repository, graph: CommitGraph, root_oid: pygit2.Oid
) -> Iterator[DisplayedCommit]:
    try:
        current = graph[root_oid]
    except KeyError:
        return

    yield current

    # Sort earlier commits first, so that they're displayed at the bottom of
    # the smartlog.
    children = sorted(current.children, key=lambda oid: repo[oid].commit_time)
    for child_oid in children:
        yield from walk_children(repo=repo, graph=graph, root_oid=child_oid)


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

    master_oid = repo.branches["master"].target

    now = int(time.time())
    num_old_commits = 0
    graph = walk_from_visible_commits(
        formatter=formatter,
        repo=repo,
        master_oid=master_oid,
        commit_oids=list(replayer.get_visible_commits()),
    )
    root_oids = split_commit_graph_by_roots(formatter=formatter, repo=repo, graph=graph)

    lines_reversed = []
    for root_oid in root_oids:
        for displayed_commit in walk_children(
            repo=repo, graph=graph, root_oid=root_oid
        ):
            oid = displayed_commit.oid
            commit = repo[oid]
            if is_commit_old(commit, now=now):
                num_old_commits += 1
                logging.debug(
                    formatter.format(
                        "Commit {oid:oid} is too old to be displayed", oid=oid
                    )
                )
            else:
                lines_reversed.append(
                    formatter.format(
                        "{oid:oid} {commit:commit}\n", oid=oid, commit=commit
                    )
                )

    lines = reversed(lines_reversed)
    for line in lines:
        out.write(line)
    if num_old_commits > 0:
        out.write(
            formatter.format(
                "({num_old_commits} old commits hidden, use --show-old to show)\n",
                num_old_commits=num_old_commits,
            )
        )
