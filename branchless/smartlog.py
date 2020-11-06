import functools
import logging
import string
import time
from dataclasses import dataclass
from queue import Queue
from typing import Dict, Iterator, List, Optional, Sequence, Set, TextIO, Tuple

import pygit2

from . import CommitStatus, Formatter, get_repo
from .glyphs import Glyphs, PrettyGlyphs, TextGlyphs
from .reflog import RefLogReplayer


def is_commit_old(commit: pygit2.Commit, now: int) -> bool:
    """Determine if a commit has not been touched for a while (is "old").

    Such commits are visible, but by default, not shown by the smartlog.
    """
    max_age = 14 * 24 * 60 * 60  # 2 weeks
    return commit.commit_time < (now - max_age)


@dataclass
class DisplayedCommit:
    commit: pygit2.Commit
    parent: Optional[pygit2.Oid]
    children: Set[pygit2.Oid]
    status: CommitStatus


CommitGraph = Dict[pygit2.Oid, DisplayedCommit]


def find_path_to_merge_base(
    formatter: Formatter,
    repo: pygit2.Repository,
    commit_oid: pygit2.Oid,
    target_oid: pygit2.Oid,
) -> List[pygit2.Commit]:
    """Find a shortest path between the given commits.

    This is particularly important for multi-parent commits (i.e. merge
    commits). If we don't happen to traverse the correct parent, we may end
    up traversing a huge amount of commit history, with a significant
    performance hit.
    """
    queue: Queue[List[pygit2.Commit]] = Queue()
    queue.put([repo[commit_oid]])
    while not queue.empty():
        path = queue.get()
        if path[-1].oid == target_oid:
            return path

        for parent in path[-1].parents:
            queue.put(path + [parent])
    raise ValueError(
        formatter.format(
            "No path between {commit_oid:oid} and {target_oid:oid}",
            commit_oid=commit_oid,
            target_oid=target_oid,
        )
    )


def walk_from_visible_commits(
    formatter: Formatter,
    repo: pygit2.Repository,
    head_oid: pygit2.Oid,
    master_oid: pygit2.Oid,
    commit_oids: Sequence[pygit2.Oid],
) -> CommitGraph:
    """Find additional commits that should be displayed.

    For example, if you check out a commit that has intermediate parent
    commits between it and `master`, those intermediate commits should be
    shown (or else you won't get a good idea of the line of development that
    happened for this commit since `master`).
    """
    graph: CommitGraph = {}

    def link(parent_oid: pygit2.Oid, child_oid: Optional[pygit2.Oid]) -> None:
        if child_oid is not None:
            graph[child_oid].parent = parent_oid
            graph[parent_oid].children.add(child_oid)

    for commit_oid in commit_oids:
        merge_base_oid = repo.merge_base(commit_oid, master_oid)
        assert merge_base_oid is not None, formatter.format(
            "No merge-base found for commits {commit_oid:oid} and {master_oid:oid}",
            commit_oid=commit_oid,
            master_oid=master_oid,
        )

        # If this was a commit directly to master, and it's not HEAD, then
        # don't show it. It's been superseded by other commits to master. Note
        # that this doesn't prohibit commits from master which are a parent of
        # a commit that we care about from being rendered.
        if commit_oid == merge_base_oid and commit_oid != head_oid:
            continue

        current_commit = repo[commit_oid]
        previous_oid = None
        for current_commit in find_path_to_merge_base(
            formatter=formatter,
            repo=repo,
            commit_oid=commit_oid,
            target_oid=merge_base_oid,
        ):
            current_oid = current_commit.oid

            if current_oid not in graph:
                graph[current_oid] = DisplayedCommit(
                    commit=current_commit, parent=None, children=set(), status="visible"
                )
                link(parent_oid=current_oid, child_oid=previous_oid)
            else:
                link(parent_oid=current_oid, child_oid=previous_oid)
                break

            previous_oid = current_oid

        if merge_base_oid in graph:
            graph[merge_base_oid].status = "master"
        else:
            logging.warning(
                formatter.format(
                    "Could not find merge base {merge_base_oid:oid}",
                    merge_base_oid=merge_base_oid,
                )
            )
    # Link any adjacent merge-bases (i.e. adjacent commits in master).
    # TODO: may not be necessary, depending on if we want to hide master
    # commits.
    for oid, displayed_commit in graph.items():
        if displayed_commit.status == "master":
            for parent in displayed_commit.commit.parents:
                if parent.oid in graph:
                    link(parent_oid=parent.oid, child_oid=displayed_commit.commit.oid)
                    break

    return graph


def split_commit_graph_by_roots(
    formatter: string.Formatter, repo: pygit2.Repository, graph: CommitGraph
) -> List[pygit2.Oid]:
    """Split fully-independent subgraphs into multiple graphs.

    This is intended to handle the situation of having multiple lines of work
    rooted from different commits in master.

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
            # lhs was topologically first, so it should be sorted earlier in the list.
            return -1
        elif merge_base == rhs:
            return 1
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


@dataclass
class ChildInfo:
    displayed_commit: DisplayedCommit
    """The commit to be displayed."""

    depth: int
    """How far from the left side this commit is.

    The left-most commit has depth 1.

    ```
    o depth 1
    |
    | o depth 2
    |/
    o depth 1
    ```
    """

    is_left_aligned: bool
    """Whether or not this commit should be left-aligned.

    A commit should be left-aligned if it's the last child of its parent. Not
    every terminal node should be left aligned, and not every left-aligned
    node is the last element in the list of children returned by
    `walk_children`.

    Normally, a commit has a depth equal to the number of parents it has,
    plus one. However, left-aligned commits have a `depth` of one less than
    their siblings. For example:

    ```
    o depth 1 (left-aligned)
    |
    | o depth 2 (left-aligned)
    | |
    | | o depth 3
    | |/
    | o depth 2
    |/
    o depth 1
    ```
    """


def walk_children(
    graph: CommitGraph,
    root_oid: pygit2.Oid,
    depth: int,
    is_left_aligned: bool,
) -> Iterator[ChildInfo]:
    """Walk children commits according to the provided graph.

    Returns useful information about the depth of each child, for later rendering.
    """
    try:
        current = graph[root_oid]
    except KeyError:
        return

    yield ChildInfo(
        displayed_commit=current, depth=depth, is_left_aligned=is_left_aligned
    )

    # Sort earlier commits first, so that they're displayed at the bottom of
    # the smartlog.
    children = sorted(current.children, key=lambda oid: graph[oid].commit.commit_time)
    for i, child_oid in enumerate(children):
        is_left_aligned = i == len(children) - 1
        if is_left_aligned:
            child_depth = depth
        else:
            child_depth = depth + 1
        yield from walk_children(
            graph=graph,
            root_oid=child_oid,
            depth=child_depth,
            is_left_aligned=is_left_aligned,
        )


@dataclass
class Output:
    lines: Sequence[str]
    num_old_commits: int


def get_output(
    glyphs: Glyphs,
    formatter: Formatter,
    graph: CommitGraph,
    head_oid: pygit2.Oid,
    root_oids: List[pygit2.Oid],
    now: int,
) -> Output:
    """Render a pretty graph starting from the given root OIDs in the given graph."""
    num_old_commits = 0
    is_first_node = True
    lines_reversed = []
    for root_idx, root_oid in enumerate(root_oids):
        children = []
        for child_info in walk_children(
            graph=graph,
            root_oid=root_oid,
            depth=1,
            is_left_aligned=False,
        ):
            commit = child_info.displayed_commit.commit
            if not is_commit_old(commit, now=now):
                children.append(child_info)
            else:
                num_old_commits += 1
                logging.debug(
                    formatter.format(
                        "Commit {commit.oid:oid} is too old to be displayed",
                        commit=commit,
                    )
                )

        for child_idx, child_info in enumerate(children):
            displayed_commit = child_info.displayed_commit
            commit = displayed_commit.commit
            depth = child_info.depth

            if child_idx == 0:
                left_line = glyphs.vertical_ellipsis
            else:
                left_line = glyphs.line

            is_left_aligned = child_info.is_left_aligned
            if is_left_aligned and root_idx != len(root_oids) - 1:
                # If there's a connection via `master` to the next root,
                # then we need to push this commit to the right to make
                # space for the line to the next root.
                is_left_aligned = False
                depth += 1
                left_line = glyphs.vertical_ellipsis

            # Print the line connecting the previous node to this node (unless
            # this is the initial commit for the repository).
            if len(commit.parents) > 0:
                left_and_middle_lines = left_line + (" " + glyphs.line) * (depth - 2)

                if child_idx == 0:
                    right_line = ""
                elif is_left_aligned:
                    if depth > 1:
                        right_line = " " + glyphs.line
                    else:
                        right_line = ""
                else:
                    right_line = glyphs.slash
                    if (
                        len(left_and_middle_lines) > 0
                        and left_and_middle_lines[-1] == glyphs.line
                    ):
                        left_and_middle_lines = (
                            left_and_middle_lines[:-1] + glyphs.line_with_offshoot
                        )

                lines_reversed.append(left_and_middle_lines + right_line)
            is_first_node = False

            # Print the current node and its commit.
            if depth == 1:
                left_line = ""
            else:
                left_line += " "

            if commit.oid == head_oid:
                cursor = glyphs.commit_head
            else:
                cursor = glyphs.commit

            lines_reversed.append(
                formatter.format(
                    "{left_line}{middle_lines}{cursor} {commit.oid:oid} {commit:commit}",
                    commit=commit,
                    left_line=left_line,
                    middle_lines=((glyphs.line + " ") * (depth - 2)),
                    cursor=cursor,
                )
            )

    lines = list(reversed(lines_reversed))
    return Output(lines=lines, num_old_commits=num_old_commits)


def smartlog(*, out: TextIO, show_old_commits: bool) -> None:
    """Display a nice graph of commits you've recently worked on."""
    formatter = Formatter()
    repo = get_repo()
    # We don't use `repo.head`, because that resolves the HEAD reference
    # (e.g. into refs/head/master). We want the actual ref-log of HEAD, not
    # the reference it points to.
    head_ref = repo.references["HEAD"]
    head_oid = head_ref.resolve().target
    replayer = RefLogReplayer(head_oid)
    for entry in head_ref.log():
        replayer.process(entry)
    replayer.finish_processing()
    visible_commit_oids = replayer.get_visible_oids()

    master_oid = repo.branches["master"].target

    graph = walk_from_visible_commits(
        formatter=formatter,
        repo=repo,
        head_oid=head_oid,
        master_oid=master_oid,
        commit_oids=visible_commit_oids,
    )
    root_oids = split_commit_graph_by_roots(formatter=formatter, repo=repo, graph=graph)

    glyphs: Glyphs
    if out.isatty() and PrettyGlyphs.ENABLED:
        glyphs = PrettyGlyphs()
    else:
        glyphs = TextGlyphs()
    output = get_output(
        glyphs=glyphs,
        formatter=formatter,
        graph=graph,
        head_oid=head_oid,
        root_oids=root_oids,
        now=int(time.time()),
    )

    for line in output.lines:
        out.write(line)
        out.write("\n")
    if output.num_old_commits > 0:
        out.write(
            formatter.format(
                "({num_old_commits} old commits hidden, use --show-old to show)\n",
                num_old_commits=output.num_old_commits,
            )
        )
