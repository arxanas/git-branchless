import functools
import logging
import string
from dataclasses import dataclass
from queue import Queue
from typing import Dict, List, Literal, Optional, Set, TextIO, Union

import colorama
import pygit2

from . import CommitStatus, get_repo
from .db import make_db_for_repo
from .formatting import Formatter, Glyphs, make_glyphs
from .hide import HideDb
from .mergebase import MergeBaseDb
from .reflog import RefLogReplayer


@dataclass
class Node:
    commit: pygit2.Commit
    parent: Optional[pygit2.Oid]
    children: Set[pygit2.Oid]
    status: CommitStatus


CommitGraph = Dict[pygit2.Oid, Node]


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
    merge_base_db: MergeBaseDb,
    head_oid: pygit2.Oid,
    master_oid: pygit2.Oid,
    visible_commit_oids: Set[pygit2.Oid],
    hidden_commit_oids: Set[str],
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

    for commit_oid in visible_commit_oids:
        merge_base_oid = merge_base_db.get_merge_base_oid(
            repo=repo, lhs_oid=commit_oid, rhs_oid=master_oid
        )
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
                status: Union[Literal["hidden"], Literal["visible"]]
                if current_oid.hex in hidden_commit_oids:
                    status = "hidden"
                else:
                    status = "visible"
                graph[current_oid] = Node(
                    commit=current_commit,
                    parent=None,
                    children=set(),
                    status=status,
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
    for oid, node in graph.items():
        if node.status == "master":
            for parent_commit in node.commit.parents:
                if parent_commit.oid in graph:
                    link(
                        parent_oid=parent_commit.oid,
                        child_oid=node.commit.oid,
                    )
                    break

    return graph


def consistency_check_graph(graph: CommitGraph) -> None:
    """Verify that each parent-child connection is mutual."""
    for node_oid, node in graph.items():
        parent_oid = node.parent
        if parent_oid is not None:
            assert parent_oid != node_oid
            assert parent_oid in graph
            assert node_oid in graph[parent_oid].children

        for child_oid in node.children:
            assert child_oid != node_oid
            assert child_oid in graph
            assert graph[child_oid].parent == node_oid


def hide_commits(graph: CommitGraph, head_oid: pygit2.Oid) -> None:
    """Hide commits according to their status.

    Commits with the hidden status should not be displayed. Additionally,
    commits descending from that commit should be not be displayed as well,
    since the user probably intended to hide the entire subtree.

    However, we want to be sure to always display the commit pointed to by
    HEAD, and its ancestry.
    """

    def should_prune(oid: pygit2.Oid) -> bool:
        if oid == head_oid:
            # Always show the HEAD commit and its children.
            return False

        node = graph[oid]
        if node.status == "hidden":
            return True
        parent = node.parent
        if parent is not None:
            return should_prune(parent)
        else:
            return False

    # Hide all subtrees which descend from a hidden commit, unless HEAD is in
    # that subtree.
    potential_oids_to_hide = {oid for oid, node in graph.items() if not node.children}
    while potential_oids_to_hide:
        next_potential_oids_to_hide = set()
        for potential_oid_to_hide in potential_oids_to_hide:
            if should_prune(potential_oid_to_hide):
                parent_oid = graph[potential_oid_to_hide].parent
                del graph[potential_oid_to_hide]
                if parent_oid is not None:
                    graph[parent_oid].children.remove(potential_oid_to_hide)
                    if not graph[parent_oid].children:
                        next_potential_oids_to_hide.add(parent_oid)
        potential_oids_to_hide = next_potential_oids_to_hide


def split_commit_graph_by_roots(
    formatter: string.Formatter,
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    graph: CommitGraph,
) -> List[pygit2.Oid]:
    """Split fully-independent subgraphs into multiple graphs.

    This is intended to handle the situation of having multiple lines of work
    rooted from different commits in master.

    Returns the list such that the topologically-earlier subgraphs are first
    in the list (i.e. those that would be rendered at the bottom of the
    smartlog).
    """
    root_commit_oids = [
        commit_oid for commit_oid, node in graph.items() if node.parent is None
    ]

    def compare(lhs: pygit2.Oid, rhs: pygit2.Oid) -> int:
        merge_base_oid = merge_base_db.get_merge_base_oid(repo, lhs, rhs)
        if merge_base_oid == lhs:
            # lhs was topologically first, so it should be sorted earlier in the list.
            return -1
        elif merge_base_oid == rhs:
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


def get_child_output(
    glyphs: Glyphs,
    formatter: Formatter,
    graph: CommitGraph,
    head_oid: pygit2.Oid,
    current_oid: pygit2.Oid,
    last_child_line_char: Optional[str],
) -> List[str]:
    current = graph[current_oid]
    text = "{oid} {message}".format(
        oid=glyphs.color_fg(
            color=colorama.Fore.YELLOW,
            message=formatter.format("{commit.oid:oid}", commit=current.commit),
        ),
        message=formatter.format("{commit:commit}", commit=current.commit),
    )

    if current.status == "hidden":
        if current.commit.oid == head_oid:
            cursor = glyphs.commit_head_hidden
        else:
            cursor = glyphs.commit_hidden
    else:
        if current.commit.oid == head_oid:
            cursor = glyphs.commit_head
        else:
            cursor = glyphs.commit
    if current.commit.oid == head_oid:
        cursor = glyphs.style(style=colorama.Style.BRIGHT, message=cursor)
        text = glyphs.style(style=colorama.Style.BRIGHT, message=text)

    lines_reversed = [f"{cursor} {text}"]

    # Sort earlier commits first, so that they're displayed at the bottom of
    # the smartlog.
    children = sorted(
        current.children, key=lambda child: graph[child].commit.commit_time
    )
    for child_idx, child_oid in enumerate(children):
        child_output = get_child_output(
            glyphs=glyphs,
            formatter=formatter,
            graph=graph,
            head_oid=head_oid,
            current_oid=child_oid,
            last_child_line_char=None,
        )

        if child_idx == len(children) - 1:
            if last_child_line_char is not None:
                lines_reversed.append(glyphs.line_with_offshoot + glyphs.slash)
            else:
                lines_reversed.append(glyphs.line)
        else:
            lines_reversed.append(glyphs.line_with_offshoot + glyphs.slash)

        for child_line in child_output:
            if child_idx == len(children) - 1:
                if last_child_line_char is not None:
                    lines_reversed.append(last_child_line_char + " " + child_line)
                else:
                    lines_reversed.append(child_line)
            else:
                lines_reversed.append(glyphs.line + " " + child_line)

    return lines_reversed


def get_output(
    glyphs: Glyphs,
    formatter: Formatter,
    graph: CommitGraph,
    head_oid: pygit2.Oid,
    root_oids: List[pygit2.Oid],
) -> List[str]:
    """Render a pretty graph starting from the given root OIDs in the given graph."""
    lines_reversed = []

    for root_idx, root_oid in enumerate(root_oids):
        root_node = graph[root_oid]
        if root_node.commit.parents:
            lines_reversed.append(
                glyphs.style(style=colorama.Style.DIM, message=glyphs.vertical_ellipsis)
            )

        last_child_line_char: Optional[str]
        if root_idx == len(root_oids) - 1:
            last_child_line_char = None
        else:
            last_child_line_char = glyphs.style(
                style=colorama.Style.DIM, message=glyphs.vertical_ellipsis
            )

        child_output = get_child_output(
            glyphs=glyphs,
            formatter=formatter,
            graph=graph,
            head_oid=head_oid,
            current_oid=root_oid,
            last_child_line_char=last_child_line_char,
        )
        lines_reversed.extend(child_output)

    return lines_reversed


def smartlog(*, out: TextIO) -> int:
    """Display a nice graph of commits you've recently worked on."""
    glyphs = make_glyphs(out)
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
    visible_commit_oids = set(replayer.get_visible_oids())

    db = make_db_for_repo(repo)
    hide_db = HideDb(db)
    hidden_commit_oids = hide_db.get_hidden_oids()

    master_oid = repo.branches["master"].target

    merge_base_db = MergeBaseDb(db)
    if merge_base_db.is_empty():
        logging.debug(
            "Merge-base cache not initialized -- it may take a while to populate it"
        )

    graph = walk_from_visible_commits(
        formatter=formatter,
        repo=repo,
        merge_base_db=merge_base_db,
        head_oid=head_oid,
        master_oid=master_oid,
        visible_commit_oids=visible_commit_oids,
        hidden_commit_oids=hidden_commit_oids,
    )
    consistency_check_graph(graph)
    hide_commits(graph=graph, head_oid=head_oid)
    consistency_check_graph(graph)

    root_oids = split_commit_graph_by_roots(
        formatter=formatter, repo=repo, merge_base_db=merge_base_db, graph=graph
    )
    lines_reversed = get_output(
        glyphs=glyphs,
        formatter=formatter,
        graph=graph,
        head_oid=head_oid,
        root_oids=root_oids,
    )

    for line in reversed(lines_reversed):
        out.write(line)
        out.write("\n")
    return 0
