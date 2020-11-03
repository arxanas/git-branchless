import logging
import time
from dataclasses import dataclass
from typing import Dict, List, Optional, Sequence, TextIO

import pygit2

from . import CommitStatus, Formatter, get_repo
from .reflog import RefLogReplayer


def is_commit_old(commit: pygit2.Commit, now: int) -> bool:
    """Determine if a commit has not been touched for a while (is "old").

    Such commits are visible, but by default, not shown by the smartlog.
    """
    # String like "-0430"
    offset_str = str(commit.commit_time_offset).zfill(5)
    offset_sign = -1 if offset_str[0] == "-" else 1
    offset_hours = int(offset_str[1:3])
    offset_minutes = int(offset_str[3:5])

    offset_seconds = offset_sign * ((offset_hours * 60 * 60) + (offset_minutes * 60))
    commit_timestamp: int = commit.commit_time
    commit_timestamp += offset_seconds
    max_age = 14 * 24 * 60 * 60  # 2 weeks
    return commit_timestamp < (now - max_age)


@dataclass
class DisplayedCommit:
    parent: pygit2.Oid
    children: List[pygit2.Oid]
    status: CommitStatus


def expand_visible_commits(
    formatter: Formatter,
    repo: pygit2.Repository,
    master_oid: pygit2.Oid,
    commit_oids: Sequence[pygit2.Oid],
) -> Dict[pygit2.Oid, DisplayedCommit]:
    """Find additional commits that should be displayed."""
    graph: Dict[pygit2.Oid, DisplayedCommit] = {}
    for commit_oid in commit_oids:
        merge_base_oid = repo.merge_base(commit_oid, master_oid)
        assert merge_base_oid is not None, formatter.format(
            "No merge-base found for commits {commit_oid:oid} and {master_oid:oid}",
            commit_oid=commit_oid,
            master_oid=master_oid,
        )
        previous_oid = commit_oid
        for current_commit in repo.walk(commit_oid, pygit2.GIT_SORT_TOPOLOGICAL):
            current_oid = current_commit.oid
            graph[current_oid] = DisplayedCommit(
                parent=previous_oid, children=[], status="visible"
            )
            if current_oid == merge_base_oid:
                graph[current_oid].status = "master"
                break
            else:
                previous_oid = current_oid
    return graph


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
    visible_commits = expand_visible_commits(
        formatter=formatter,
        repo=repo,
        master_oid=master_oid,
        commit_oids=list(replayer.get_visible_commits()),
    )
    for oid, _display_info in visible_commits.items():
        commit: pygit2.Commit = repo[oid]
        if is_commit_old(commit, now=now):
            num_old_commits += 1
            logging.debug(
                formatter.format("Commit {oid:oid} is too old to be displayed", oid=oid)
            )
        else:
            out.write(
                formatter.format("{oid:oid} {commit:commit}\n", oid=oid, commit=commit)
            )
    if num_old_commits > 0:
        out.write(
            formatter.format(
                "({num_old_commits} old commits hidden, use --show-old to show)\n",
                num_old_commits=num_old_commits,
            )
        )
