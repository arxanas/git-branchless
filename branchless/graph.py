import functools
import logging
from dataclasses import dataclass
from queue import Queue
from typing import Dict, List, Optional, Set

import pygit2

from . import OidStr
from .eventlog import Event, EventReplayer
from .mergebase import MergeBaseDb


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

    is_main: bool
    """Indicates that this is a commit to the main branch.

    These commits are considered to be immutable and should never leave the
    `main` state. However, this can still happen sometimes if the user's
    workflow is different than expected.
    """

    is_visible: bool
    """Indicates that this commit should be considered "visible".

    A visible commit is a commit that hasn't been checked into the main
    branch, but the user is actively working on. We may infer this from user
    behavior, e.g. they committed something recently, so they are now working
    on it.

    In contrast, a hidden commit is a commit that hasn't been checked into
    the main branch, and the user is no longer working on. We may infer this
    from user behavior, e.g. they have rebased a commit and no longer want to
    see the old version of that commit. The user can also manually hide
    commits.

    Occasionally, a main commit can be marked as hidden, such as if a commit
    in the main branch has been rewritten. We don't expect this to happen in
    the monorepo workflow, but it can happen in other workflows where you
    commit directly to the main branch and then later rewrite the commit.
    """

    event: Optional[Event]
    """The latest event to affect this commit.

    It's possible that no event affected this commit, and it was simply
    visible due to a reference pointing to it. In that case, this field is
    `None`.
    """


CommitGraph = Dict[OidStr, Node]
"""Graph of commits that the user is working on."""


def find_path_to_merge_base(
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    commit_oid: pygit2.Oid,
    target_oid: pygit2.Oid,
) -> Optional[List[pygit2.Commit]]:
    """Find a shortest path between the given commits.

    This is particularly important for multi-parent commits (i.e. merge
    commits). If we don't happen to traverse the correct parent, we may end
    up traversing a huge amount of commit history, with a significant
    performance hit.

    Args:
      repo: The Git repository.
      commit_oid: The OID of the commit to start at. We take parents of the
        provided commit until we end up at the target OID.
      target_oid: The OID of the commit to end at.

    Returns:
      A path of commits from `commit_oid` through parents to `target_oid`.
      The path includes `commit_oid` at the beginning and `target_oid` at the
      end. If there is no such path, returns `None`.
    """
    queue: Queue[List[pygit2.Commit]] = Queue()
    queue.put([repo[commit_oid]])
    merge_base_oid = merge_base_db.get_merge_base_oid(
        repo=repo, lhs_oid=commit_oid, rhs_oid=target_oid
    )
    while not queue.empty():
        path = queue.get()
        if path[-1].oid == target_oid:
            return path
        if path[-1].oid == merge_base_oid:
            # We've hit the common ancestor of these two commits without
            # finding a path between them. That means it's impossible to find a
            # path between them by traversing more ancestors. Possibly the
            # caller passed them in in the wrong order, i.e. `commit_oid` is
            # actually a parent of `target_oid`.
            continue

        for parent in path[-1].parents:
            # For test: access the parent commit through `repo` so that we can
            # track it.
            parent = repo[parent.oid]

            queue.put(path + [parent])
    return None


def _walk_from_commits(
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    event_replayer: EventReplayer,
    head_oid: Optional[OidStr],
    main_branch_oid: pygit2.Oid,
    branch_oids: Set[OidStr],
    commit_oids: Set[OidStr],
) -> CommitGraph:
    """Find additional commits that should be displayed.

    For example, if you check out a commit that has intermediate parent
    commits between it and the main branch, those intermediate commits should be
    shown (or else you won't get a good idea of the line of development that
    happened for this commit since the main branch).
    """
    graph: CommitGraph = {}

    for commit_oid_hex in commit_oids:
        current_commit = repo[commit_oid_hex]
        merge_base_oid = merge_base_db.get_merge_base_oid(
            repo=repo, lhs_oid=current_commit.oid, rhs_oid=main_branch_oid
        )

        # Occasionally we may find a commit that has no merge-base with the
        # main branch. For example: a rewritten initial commit. This is
        # somewhat pathological. We'll just add it to the graph as a standalone
        # component and hope it works out.
        if merge_base_oid is None:
            path_to_merge_base: List[pygit2.Commit] = [current_commit]
        else:
            path_to_merge_base_opt = find_path_to_merge_base(
                repo=repo,
                merge_base_db=merge_base_db,
                commit_oid=current_commit.oid,
                target_oid=merge_base_oid,
            )
            if path_to_merge_base_opt is None:
                # All visible commits should be rooted in the main branch, so
                # this shouldn't happen.
                logging.warning(
                    "No path to merge-base for commit %s", current_commit.oid
                )
                continue
            path_to_merge_base = path_to_merge_base_opt

        for current_commit in path_to_merge_base:
            current_oid = current_commit.oid.hex
            if current_oid in graph:
                # This commit (and all of its parents!) should be in the graph
                # already, so no need to continue this iteration.
                break

            visibility = event_replayer.get_commit_visibility(current_oid)
            if visibility is None or visibility == "visible":
                is_visible = True
            elif visibility == "hidden":
                is_visible = False

            if merge_base_oid is not None:
                is_main = current_oid == merge_base_oid.hex
            else:
                is_main = False

            event = event_replayer.get_commit_latest_event(current_oid)
            graph[current_oid] = Node(
                commit=current_commit,
                parent=None,
                children=set(),
                is_main=is_main,
                is_visible=is_visible,
                event=event,
            )

        if merge_base_oid is not None and merge_base_oid.hex not in graph:
            logging.warning(
                f"Could not find merge base {merge_base_oid}",
            )

    def link(parent_oid: OidStr, child_oid: OidStr) -> None:
        graph[child_oid].parent = parent_oid
        graph[parent_oid].children.add(child_oid)

    for oid, node in graph.items():
        if node.is_main:
            continue
        for parent_oid in node.commit.parent_ids:
            if parent_oid.hex in graph:
                link(parent_oid=parent_oid.hex, child_oid=oid)

    return graph


def _hide_commits(
    graph: CommitGraph,
    event_replayer: EventReplayer,
    head_oid: Optional[OidStr],
    branch_oids: Set[OidStr],
) -> None:
    """Remove commits from the graph according to their status."""
    # OIDs which are pointed to by HEAD or a branch should not be hidden.
    # Therefore, we can't hide them *or* their ancestors.
    unhideable_oids = set(branch_oids)
    if head_oid is not None:
        unhideable_oids.add(head_oid)

    # Hide any subtrees which are entirely hidden.
    @functools.lru_cache
    def should_hide(oid: OidStr) -> bool:
        if oid in unhideable_oids:
            return False

        node = graph[oid]

        if node.is_main:
            # We only want to hide "uninteresting" main branch nodes. Main
            # branch nodes should normally be visible, so instead, we only hide
            # it if it's *not* visible, which is an anomaly that should be
            # addressed by the user.
            return node.is_visible and all(
                should_hide(child_oid)
                for child_oid in node.children
                # Don't consider the next commit in the main branch as a child
                # for hiding purposes.
                if not graph[child_oid].is_main
            )
        else:
            return not node.is_visible and all(
                should_hide(child_oid) for child_oid in node.children
            )

    all_oids_to_hide = {oid for oid in graph.keys() if should_hide(oid)}

    # Actually update the graph and delete any parent-child links, as
    # appropriate.
    for oid in all_oids_to_hide:
        parent_oid = graph[oid].parent
        del graph[oid]
        if parent_oid is not None and parent_oid in graph:
            graph[parent_oid].children.remove(oid)


def make_graph(
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    event_replayer: EventReplayer,
    head_oid: Optional[OidStr],
    main_branch_oid: pygit2.Oid,
    branch_oids: Set[OidStr],
    hide_commits: bool,
) -> CommitGraph:
    """Construct the smartlog graph for the repo.

    Args:
      repo: The Git repository.
      merge_base_db: The merge-base database.
      event_replayer: The event replayer.
      head_oid: The OID of the repository's `HEAD` reference.
      main_branch_oid: The OID of the main branch.
      branch_oids: The set of OIDs pointed to by branches.
      hide_commits: If set to `True`, then, after constructing the graph,
        remove nodes from it that appear to be hidden by user activity. This
        should be set to `True` for most display-related purposes.

    Returns:
      A tuple of the head OID and the commit graph.
    """
    commit_oids = event_replayer.get_active_oids()
    commit_oids.update(branch_oids)
    if head_oid is not None:
        commit_oids.add(head_oid)

    graph = _walk_from_commits(
        repo=repo,
        merge_base_db=merge_base_db,
        event_replayer=event_replayer,
        head_oid=head_oid,
        main_branch_oid=main_branch_oid,
        branch_oids=branch_oids,
        commit_oids=commit_oids,
    )
    if hide_commits:
        _hide_commits(
            graph=graph,
            event_replayer=event_replayer,
            branch_oids=branch_oids,
            head_oid=head_oid,
        )
    return graph
