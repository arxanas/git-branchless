import builtins
from typing import Literal, Optional, TextIO, Union

from . import get_repo, run_git
from .db import make_db_for_repo
from .eventlog import EventLogDb
from .formatting import make_glyphs
from .graph import make_graph
from .mergebase import MergeBaseDb
from .metadata import CommitMessageProvider, CommitOidProvider, render_commit_metadata
from .smartlog import smartlog


def prev(out: TextIO, num_commits: Optional[int]) -> int:
    if num_commits is None:
        result = run_git(out=out, args=["checkout", "HEAD^"])
    else:
        result = run_git(out=out, args=["checkout", f"HEAD~{num_commits}"])
    if result != 0:
        return result

    return smartlog(out=out)


def next(
    out: TextIO,
    num_commits: Optional[int],
    towards: Optional[Union[Literal["newest"], Literal["oldest"]]],
) -> int:
    glyphs = make_glyphs(out)
    repo = get_repo()
    db = make_db_for_repo(repo)
    merge_base_db = MergeBaseDb(db)
    event_log_db = EventLogDb(db)
    graph_result = make_graph(
        repo=repo,
        merge_base_db=merge_base_db,
        event_log_db=event_log_db,
    )
    head_oid = graph_result.head_oid
    graph = graph_result.graph

    if num_commits is None:
        num_commits_ = 1
    else:
        num_commits_ = num_commits

    current_oid = head_oid.hex
    for i in range(num_commits_):
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
            return 1

    result = run_git(out=out, args=["checkout", current_oid])
    if result != 0:
        return result

    return smartlog(out=out)
