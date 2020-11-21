import logging
from dataclasses import dataclass
from queue import Queue
from typing import Dict, List, Literal, Optional, Set, Union

import pygit2

from .eventlog import Event, EventLogDb, EventReplayer, HideEvent, OidStr
from .mergebase import MergeBaseDb

CommitStatus = Union[Literal["master"], Literal["visible"], Literal["hidden"]]
"""The possible states a commit can be in.

  * `master`: a commit to the `master` branch. These are considered to be
  immutable and will never leave the `master` state.
  * `visible`: a commit that hasn't been checked into master, but the user is
  actively working on. We may infer this from user behavior, e.g. they
  committed something recently, so they are now working on it.
  * `hidden`: a commit that hasn't been checked into master, and the user is no
  longer working on. We may infer this from user behavior, e.g. they have
  rebased a commit and no longer want to see the old version of that commit.
  The user can also manually hide commits.

Commits can transition between `visible` and `hidden` depending on user
behavior, but commits never transition to or from `master`. (It's assumed
that commits are added to `master` only via pulling from the remote.)
"""


@dataclass
class Node:
    """Node contained in the smartlog commit graph."""

    commit: pygit2.Commit
    """The underlying commit object."""

    parent: Optional[OidStr]
    """The OID of the parent node in the smartlog commit graph.

    This is different from inspecting `commit.parents`, since the smartlog
    will hide most nodes from the commit graph, including parent nodes.
    """

    children: Set[OidStr]
    """The OIDs of the children nodes in the smartlog commit graph."""

    status: CommitStatus
    """The status of this commit."""

    event: Optional[Event]
    """The latest event to affect this commit.

    It's possible that no event affected this commit, and it was simply
    visible due to a reference pointing to it. In that case, this field is
    `None`.
    """


CommitGraph = Dict[OidStr, Node]
"""Graph of commits that the user is working on."""


def _find_path_to_merge_base(
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
        f"No path between {commit_oid} and {target_oid}",
    )


def _walk_from_visible_commits(
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    event_replayer: EventReplayer,
    branch_oids: Set[OidStr],
    head_oid: pygit2.Oid,
    master_oid: pygit2.Oid,
    visible_commit_oids: Set[OidStr],
) -> CommitGraph:
    """Find additional commits that should be displayed.

    For example, if you check out a commit that has intermediate parent
    commits between it and `master`, those intermediate commits should be
    shown (or else you won't get a good idea of the line of development that
    happened for this commit since `master`).
    """
    graph: CommitGraph = {}

    def link(parent_oid: OidStr, child_oid: Optional[OidStr]) -> None:
        if child_oid is not None:
            graph[child_oid].parent = parent_oid
            graph[parent_oid].children.add(child_oid)

    for commit_oid_hex in visible_commit_oids:
        commit_oid = repo[commit_oid_hex].oid
        merge_base_oid = merge_base_db.get_merge_base_oid(
            repo=repo, lhs_oid=commit_oid, rhs_oid=master_oid
        )

        # Occasionally we may find a commit that has no merge-base with
        # `master`. For example: a rewritten initial commit. This is somewhat
        # pathological. We'll just handle it by not rendering it.
        if merge_base_oid is None:
            continue

        # If this was a commit directly to master, and it's not HEAD, then
        # don't show it. It's been superseded by other commits to master. Note
        # that this doesn't prohibit commits from master which are a parent of
        # a commit that we care about from being rendered.
        if commit_oid == merge_base_oid and (
            commit_oid != head_oid and commit_oid.hex not in branch_oids
        ):
            continue

        current_commit = repo[commit_oid]
        previous_oid = None
        for current_commit in _find_path_to_merge_base(
            repo=repo,
            commit_oid=commit_oid,
            target_oid=merge_base_oid,
        ):
            current_oid = current_commit.oid.hex

            if current_oid not in graph:
                status = event_replayer.get_commit_visibility(current_oid)
                if status is None:
                    status = "visible"
                event = event_replayer.get_commit_latest_event(current_oid)
                graph[current_oid] = Node(
                    commit=current_commit,
                    parent=None,
                    children=set(),
                    status=status,
                    event=event,
                )
                link(parent_oid=current_oid, child_oid=previous_oid)
            else:
                link(parent_oid=current_oid, child_oid=previous_oid)
                break

            previous_oid = current_oid

        if merge_base_oid.hex in graph:
            graph[merge_base_oid.hex].status = "master"
        else:
            logging.warning(
                f"Could not find merge base {merge_base_oid}",
            )

    return graph


def _consistency_check_graph(graph: CommitGraph) -> None:
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


def _hide_commits(
    graph: CommitGraph,
    event_replayer: EventReplayer,
    branch_oids: Set[OidStr],
    head_oid: pygit2.Oid,
) -> None:
    """Remove commits from the graph according to their status."""
    # OIDs which are pointed to by HEAD or a branch should not be hidden.
    # Therefore, we can't hide them *or* their ancestors.
    unhideable_oids = set()
    for unhideable_oid in branch_oids | {head_oid.hex}:
        while unhideable_oid in graph:
            unhideable_oids.add(unhideable_oid)
            parent = graph[unhideable_oid].parent
            if parent is None:
                break
            unhideable_oid = parent

    # Recursively hide children of commits that have been explicitly marked as
    # hidden by the user. However, this doesn't apply to commits that were
    # hidden due to a rewrite.
    all_oids_to_hide = set()
    current_oids_to_hide = {
        oid
        for oid, node in graph.items()
        if node.status == "hidden" and isinstance(node.event, HideEvent)
    }
    while current_oids_to_hide:
        all_oids_to_hide.update(current_oids_to_hide)
        next_oids_to_hide = set()
        for oid in current_oids_to_hide:
            next_oids_to_hide.update(graph[oid].children)
        current_oids_to_hide = next_oids_to_hide

    # Master nodes whose children are all hidden should also be hidden.
    # Otherwise, we get sequences of master nodes that appear in the graph for
    # no apparent reason, simply because they're technically "visible".
    for oid, node in graph.items():
        if node.status == "master" and node.children.issubset(all_oids_to_hide):
            all_oids_to_hide.add(oid)

    # Actually update the graph and delete any parent-child links, as
    # appropriate.
    all_oids_to_hide.difference_update(unhideable_oids)
    for oid in all_oids_to_hide:
        parent_oid = graph[oid].parent
        del graph[oid]
        if parent_oid is not None and parent_oid in graph:
            graph[parent_oid].children.remove(oid)


@dataclass
class GraphResult:
    head_oid: pygit2.Oid
    graph: CommitGraph
    event_replayer: EventReplayer


def make_graph(
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    event_log_db: EventLogDb,
) -> GraphResult:
    """Construct the smartlog graph for the repo.

    Args:
      repo: The Git repository.
      merge_base_db: The merge-base database.
      event_log_db:  The event log database.

    Returns:
      A tuple of the head OID and the commit graph.
    """

    # We don't use `repo.head`, because that resolves the HEAD reference
    # (e.g. into refs/head/master). We want the actual ref-log of HEAD, not
    # the reference it points to.
    head_ref = repo.references["HEAD"]
    head_oid = head_ref.resolve().target

    event_replayer = EventReplayer()
    for event in event_log_db.get_events():
        event_replayer.process_event(event)
    visible_commit_oids = event_replayer.get_visible_oids()

    branch_oids = set(
        repo.branches[branch_name].target.hex
        for branch_name in repo.listall_branches(pygit2.GIT_BRANCH_LOCAL)
    )
    visible_commit_oids.update(branch_oids)
    visible_commit_oids.add(head_oid.hex)

    master_oid = repo.branches["master"].target

    graph = _walk_from_visible_commits(
        repo=repo,
        merge_base_db=merge_base_db,
        event_replayer=event_replayer,
        branch_oids=branch_oids,
        head_oid=head_oid,
        master_oid=master_oid,
        visible_commit_oids=visible_commit_oids,
    )
    _consistency_check_graph(graph)
    _hide_commits(
        graph=graph,
        event_replayer=event_replayer,
        branch_oids=branch_oids,
        head_oid=head_oid,
    )
    _consistency_check_graph(graph)
    return GraphResult(head_oid=head_oid, graph=graph, event_replayer=event_replayer)
