"""Display a graph of commits that the user has worked on recently.

The set of commits that are still being worked on is inferred from the
ref-log; see the `reflog` module.
"""
import functools
import time
from typing import List, Optional, TextIO

import colorama
import pygit2

from . import (
    OidStr,
    get_branch_oid_to_names,
    get_head_oid,
    get_main_branch_oid,
    get_repo,
)
from .db import make_db_for_repo
from .eventlog import EventLogDb, EventReplayer
from .formatting import Glyphs, make_glyphs
from .graph import CommitGraph, make_graph
from .mergebase import MergeBaseDb
from .metadata import (
    BranchesProvider,
    CommitMessageProvider,
    CommitMetadataProvider,
    CommitOidProvider,
    DifferentialRevisionProvider,
    RelativeTimeProvider,
    render_commit_metadata,
)


def _split_commit_graph_by_roots(
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    graph: CommitGraph,
) -> List[OidStr]:
    """Split fully-independent subgraphs into multiple graphs.

    This is intended to handle the situation of having multiple lines of work
    rooted from different commits in the main branch.

    Returns the list such that the topologically-earlier subgraphs are first
    in the list (i.e. those that would be rendered at the bottom of the
    smartlog).
    """
    root_commit_oids = [
        commit_oid for commit_oid, node in graph.items() if node.parent is None
    ]

    def compare(lhs: OidStr, rhs: OidStr) -> int:
        lhs_commit = repo[lhs]
        lhs_oid = lhs_commit.oid
        rhs_commit = repo[rhs]
        rhs_oid = rhs_commit.oid
        merge_base_oid = merge_base_db.get_merge_base_oid(repo, lhs_oid, rhs_oid)
        if merge_base_oid == lhs_oid:
            # lhs was topologically first, so it should be sorted earlier in the list.
            return -1
        elif merge_base_oid == rhs_oid:
            return 1

        # The commits were not orderable (pathlogical situation). Let's
        # just order them by timestamp in that case to produce a consistent
        # and reasonable guess at the intended topological ordering.
        if lhs_commit.commit_time < rhs_commit.commit_time:
            return -1
        elif lhs_commit.commit_time > rhs_commit.commit_time:
            return 1

        # Commits produced by automation may have the same timestamp, so pick
        # any other consistent ordering.
        if lhs_oid.hex < rhs_oid.hex:
            return -1
        elif lhs_oid.hex > rhs_oid.hex:
            return 1
        else:
            return 0

    root_commit_oids.sort(key=functools.cmp_to_key(compare))
    return root_commit_oids


def _get_child_output(
    glyphs: Glyphs,
    graph: CommitGraph,
    root_oids: List[OidStr],
    commit_metadata_providers: List[CommitMetadataProvider],
    head_oid: Optional[OidStr],
    current_oid: OidStr,
    last_child_line_char: Optional[str],
) -> List[str]:
    current = graph[current_oid]
    text = render_commit_metadata(
        glyphs=glyphs,
        commit=current.commit,
        commit_metadata_providers=commit_metadata_providers,
    )

    cursor = {
        (False, False, False): glyphs.commit_hidden,
        (False, False, True): glyphs.commit_hidden_head,
        (False, True, False): glyphs.commit_visible,
        (False, True, True): glyphs.commit_visible_head,
        (True, False, False): glyphs.commit_main_hidden,
        (True, False, True): glyphs.commit_main_hidden_head,
        (True, True, False): glyphs.commit_main,
        (True, True, True): glyphs.commit_main_head,
    }[(current.is_main, current.is_visible, current.commit.oid.hex == head_oid)]
    if current.commit.oid.hex == head_oid:
        cursor = glyphs.style(style=colorama.Style.BRIGHT, message=cursor)
        text = glyphs.style(style=colorama.Style.BRIGHT, message=text)

    lines = [f"{cursor} {text}"]

    # Sort earlier commits first, so that they're displayed at the bottom of
    # the smartlog.
    children = sorted(
        current.children, key=lambda child: graph[child].commit.commit_time
    )
    for child_idx, child_oid in enumerate(children):
        # Will be rendered by the parent.
        if child_oid in root_oids:
            continue

        child_output = _get_child_output(
            glyphs=glyphs,
            graph=graph,
            root_oids=root_oids,
            commit_metadata_providers=commit_metadata_providers,
            head_oid=head_oid,
            current_oid=child_oid,
            last_child_line_char=None,
        )

        if child_idx == len(children) - 1:
            if last_child_line_char is not None:
                lines.append(glyphs.line_with_offshoot + glyphs.slash)
            else:
                lines.append(glyphs.line)
        else:
            lines.append(glyphs.line_with_offshoot + glyphs.slash)

        for child_line in child_output:
            if child_idx == len(children) - 1:
                if last_child_line_char is not None:
                    lines.append(last_child_line_char + " " + child_line)
                else:
                    lines.append(child_line)
            else:
                lines.append(glyphs.line + " " + child_line)

    return lines


def _get_output(
    glyphs: Glyphs,
    graph: CommitGraph,
    commit_metadata_providers: List[CommitMetadataProvider],
    head_oid: Optional[OidStr],
    root_oids: List[OidStr],
) -> List[str]:
    """Render a pretty graph starting from the given root OIDs in the given graph."""
    lines = []

    def has_real_parent(oid: OidStr, parent_oid: OidStr) -> bool:
        """Determine if the provided OID has the provided parent OID as a parent.

        This returns `True` in strictly more cases than checking `graph`,
        since there may be links between adjacent main branch commits which
        are not reflected in `graph`.
        """
        return any(parent.hex == parent_oid for parent in graph[oid].commit.parent_ids)

    for root_idx, root_oid in enumerate(root_oids):
        root_node = graph[root_oid]
        if len(root_node.commit.parent_ids) > 0:
            if root_idx > 0 and has_real_parent(
                oid=root_oid, parent_oid=root_oids[root_idx - 1]
            ):
                lines.append(glyphs.line)
            else:
                lines.append(glyphs.vertical_ellipsis)
        elif root_idx > 0:
            # Pathological case: multiple topologically-unrelated roots.
            # Separate them with a newline.
            lines.append("")

        last_child_line_char: Optional[str]
        if root_idx == len(root_oids) - 1:
            last_child_line_char = None
        else:
            next_root_oid = root_oids[root_idx + 1]
            if has_real_parent(oid=next_root_oid, parent_oid=root_oid):
                last_child_line_char = glyphs.line
            else:
                last_child_line_char = glyphs.vertical_ellipsis

        child_output = _get_child_output(
            glyphs=glyphs,
            graph=graph,
            root_oids=root_oids,
            commit_metadata_providers=commit_metadata_providers,
            head_oid=head_oid,
            current_oid=root_oid,
            last_child_line_char=last_child_line_char,
        )
        lines.extend(child_output)

    return lines


def render_graph(
    out: TextIO,
    glyphs: Glyphs,
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    graph: CommitGraph,
    head_oid: Optional[OidStr],
    commit_metadata_providers: List[CommitMetadataProvider],
) -> None:
    root_oids = _split_commit_graph_by_roots(
        repo=repo, merge_base_db=merge_base_db, graph=graph
    )
    lines = _get_output(
        glyphs=glyphs,
        graph=graph,
        commit_metadata_providers=commit_metadata_providers,
        head_oid=head_oid,
        root_oids=root_oids,
    )

    for line in lines:
        if out.isatty():
            out.write("\033[K")
        out.write(line)
        out.write("\n")


def smartlog(*, out: TextIO) -> int:
    """Display a nice graph of commits you've recently worked on.

    Args:
      out: The output stream to write to.

    Returns:
      Exit code (0 denotes successful exit).
    """
    glyphs = make_glyphs(out)

    repo = get_repo()
    main_branch_oid = get_main_branch_oid(repo)

    db = make_db_for_repo(repo)
    event_log_db = EventLogDb(db)
    merge_base_db = MergeBaseDb(db)
    event_replayer = EventReplayer.from_event_log_db(event_log_db)
    head_oid = get_head_oid(repo)
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

    render_graph(
        out=out,
        glyphs=glyphs,
        repo=repo,
        merge_base_db=merge_base_db,
        graph=graph,
        head_oid=head_oid.hex,
        commit_metadata_providers=[
            CommitOidProvider(glyphs=glyphs, use_color=True),
            RelativeTimeProvider(glyphs=glyphs, repo=repo, now=int(time.time())),
            BranchesProvider(
                glyphs=glyphs,
                repo=repo,
                branch_oid_to_names=branch_oid_to_names,
            ),
            DifferentialRevisionProvider(glyphs=glyphs, repo=repo),
            CommitMessageProvider(),
        ],
    )
    return 0
