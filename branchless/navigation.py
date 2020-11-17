import builtins
import subprocess
from typing import Literal, Optional, TextIO, Union

from . import get_repo
from .db import make_db_for_repo
from .eventlog import EventLogDb
from .formatting import Formatter, make_glyphs
from .mergebase import MergeBaseDb
from .smartlog import make_graph, smartlog


def _git_checkout(out: TextIO, target: str) -> int:
    out.write(f"branchless: git checkout {target}\n")
    result = subprocess.run(["git", "checkout", target], stdout=out)
    return result.returncode


def prev(out: TextIO, num_commits: Optional[int]) -> int:
    if num_commits is None:
        result = _git_checkout(out=out, target="HEAD^")
    else:
        result = _git_checkout(out=out, target=f"HEAD~{num_commits}")
    if result != 0:
        return result

    return smartlog(out=out)


def next(
    out: TextIO,
    num_commits: Optional[int],
    towards: Optional[Union[Literal["newest"], Literal["oldest"]]],
) -> int:
    formatter = Formatter()
    glyphs = make_glyphs(out)
    repo = get_repo()
    db = make_db_for_repo(repo)
    merge_base_db = MergeBaseDb(db)
    event_log_db = EventLogDb(db)
    (head_oid, graph) = make_graph(
        formatter=formatter,
        repo=repo,
        merge_base_db=merge_base_db,
        event_log_db=event_log_db,
    )

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
                out.write(
                    formatter.format(
                        "  {bullet} {commit.oid:oid} {commit:commit}{descriptor}\n",
                        bullet=glyphs.bullet_point,
                        commit=repo[child_oid],
                        descriptor=descriptor,
                    )
                )
            out.write(
                "(Pass --oldest (-o) or --newest (-n) to select between ambiguous next commits)\n"
            )
            return 1

    result = _git_checkout(out=out, target=current_oid)
    if result != 0:
        return result

    return smartlog(out=out)
