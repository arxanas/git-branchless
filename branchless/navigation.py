import builtins
import logging
from typing import Literal, Optional, TextIO, Tuple, Union

import pygit2

from . import get_repo, run_git
from .db import make_db_for_repo
from .eventlog import EventLogDb, EventReplayer, OidStr
from .formatting import Glyphs, make_glyphs
from .graph import CommitGraph, find_path_to_merge_base, get_main_branch_oid, make_graph
from .mergebase import MergeBaseDb
from .metadata import CommitMessageProvider, CommitOidProvider, render_commit_metadata
from .smartlog import smartlog

Towards = Optional[Union[Literal["newest"], Literal["oldest"]]]


def prev(out: TextIO, err: TextIO, num_commits: Optional[int]) -> int:
    if num_commits is None:
        result = run_git(out=out, err=err, args=["checkout", "HEAD^"])
    else:
        result = run_git(out=out, err=err, args=["checkout", f"HEAD~{num_commits}"])
    if result != 0:
        return result

    return smartlog(out=out)


def _advance_towards_main_branch(
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    graph: CommitGraph,
    current_oid: pygit2.Oid,
    main_branch_oid: pygit2.Oid,
    num_commits: int,
) -> Tuple[int, pygit2.Oid]:
    path = find_path_to_merge_base(
        repo=repo,
        merge_base_db=merge_base_db,
        target_oid=current_oid,
        commit_oid=main_branch_oid,
    )
    if path is None:
        return (0, current_oid)
    if len(path) == 1:
        # Must be the case that `current_oid == main_branch_oid`.
        return (0, current_oid)

    for i, commit in enumerate(path[-2::-1], start=1):
        if commit.oid.hex in graph:
            return (i, commit.oid)

    # The `main_branch_oid` commit itself should be in `graph`, so we should always
    # find a commit.
    logging.warning(
        "Failed to find graph commit when advancing towards main branch"
    )  # pragma: no cover
    return (0, current_oid)  # pragma: no cover


def _advance_towards_own_commit(
    out: TextIO,
    glyphs: Glyphs,
    repo: pygit2.Repository,
    graph: CommitGraph,
    current_oid: OidStr,
    num_commits: int,
    towards: Towards,
) -> Optional[OidStr]:
    for i in range(num_commits):
        children = list(graph[current_oid].children)
        children.sort(key=lambda child_oid: graph[child_oid].commit.commit_time)
        if len(children) == 0:
            break
        elif len(children) == 1:
            current_oid = builtins.next(iter(children))
        elif towards == "newest":
            current_oid = children[-1]
        elif towards == "oldest":
            current_oid = children[0]
        else:
            out.write(
                f"Found multiple possible next commits to go to after traversing {i} children:\n"
            )

            for i, child_oid in enumerate(children):
                if i == 0:
                    descriptor = " (oldest)"
                elif i == len(children) - 1:
                    descriptor = " (newest)"
                else:
                    descriptor = ""

                commit_text = render_commit_metadata(
                    glyphs=glyphs,
                    commit=repo[child_oid],
                    commit_metadata_providers=[
                        CommitOidProvider(glyphs=glyphs, use_color=True),
                        CommitMessageProvider(),
                    ],
                )
                out.write(
                    f"  {glyphs.bullet_point} {commit_text}{descriptor}\n",
                )
            out.write(
                "(Pass --oldest (-o) or --newest (-n) to select between ambiguous next commits)\n"
            )
            return None
    return current_oid


def next(out: TextIO, err: TextIO, num_commits: Optional[int], towards: Towards) -> int:
    glyphs = make_glyphs(out)
    repo = get_repo()
    db = make_db_for_repo(repo)
    merge_base_db = MergeBaseDb(db)
    event_log_db = EventLogDb(db)
    event_replayer = EventReplayer.from_event_log_db(event_log_db)

    main_branch_oid = get_main_branch_oid(repo)
    (head_oid, graph) = make_graph(
        repo=repo,
        merge_base_db=merge_base_db,
        event_replayer=event_replayer,
        main_branch_oid=main_branch_oid,
        hide_commits=True,
    )

    if num_commits is None:
        num_commits_ = 1
    else:
        num_commits_ = num_commits

    (
        num_commits_traversed_towards_main_branch,
        current_oid,
    ) = _advance_towards_main_branch(
        repo=repo,
        merge_base_db=merge_base_db,
        graph=graph,
        main_branch_oid=main_branch_oid,
        current_oid=head_oid,
        num_commits=num_commits_,
    )
    num_commits_ -= num_commits_traversed_towards_main_branch
    current_oid_str = _advance_towards_own_commit(
        out=out,
        glyphs=glyphs,
        repo=repo,
        graph=graph,
        current_oid=current_oid.hex,
        num_commits=num_commits_,
        towards=towards,
    )
    if current_oid_str is None:
        return 1

    result = run_git(out=out, err=err, args=["checkout", current_oid_str])
    if result != 0:
        return result

    return smartlog(out=out)
