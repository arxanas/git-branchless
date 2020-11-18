import collections
import re
from typing import Callable, Optional, List

import pygit2
import colorama

from .formatting import Glyphs, Formatter

CommitMetadataProvider = Callable[[pygit2.Oid], Optional[str]]


class BranchesProvider:
    def __init__(self, glyphs: Glyphs, repo: pygit2.Repository) -> None:
        self._glyphs = glyphs
        oid_to_branches = collections.defaultdict(list)
        for branch_name in repo.listall_branches(pygit2.GIT_BRANCH_LOCAL):
            branch = repo.branches[branch_name]
            oid_to_branches[branch.resolve().target].append(branch)
        self._oid_to_branches = oid_to_branches

    def __call__(self, oid: pygit2.Oid) -> Optional[str]:
        branches = self._oid_to_branches[oid]
        if branches:
            return self._glyphs.color_fg(
                color=colorama.Fore.GREEN,
                message="("
                + ", ".join(sorted(branch.branch_name for branch in branches))
                + ")",
            )
        else:
            return None


class DifferentialRevisionProvider:
    RE = re.compile(
        r"""
^
Differential[ ]Revision .+ / (?P<diff>D[0-9]+)
$
""",
        re.VERBOSE | re.MULTILINE,
    )

    def __init__(self, repo: pygit2.Repository) -> None:
        self._repo = repo

    def __call__(self, oid: pygit2.Oid) -> Optional[str]:
        commit_message = self._repo[oid].message
        match = self.RE.search(commit_message)
        if match is not None:
            return match.group("diff")
        else:
            return None


def get_commit_metadata(
    formatter: Formatter,
    glyphs: Glyphs,
    commit_metadata_providers: List[CommitMetadataProvider],
    oid: pygit2.Oid,
) -> Optional[str]:
    metadata_list: List[Optional[str]] = [
        provider(oid) for provider in commit_metadata_providers
    ]
    metadata_list_: List[str] = [
        metadata + " " for metadata in metadata_list if metadata is not None
    ]
    if metadata_list_:
        return "".join(metadata_list_)
    else:
        return None
