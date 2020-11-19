import collections
import re
from typing import Callable, Optional, List

import pygit2
import colorama

from .eventlog import OidStr
from .formatting import Glyphs, Formatter

CommitMetadataProvider = Callable[[OidStr], Optional[str]]


def get_commit_metadata(
    formatter: Formatter,
    glyphs: Glyphs,
    commit_metadata_providers: List[CommitMetadataProvider],
    oid: OidStr,
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


def _is_enabled(repo: pygit2.Repository, name: str, default: bool) -> bool:
    name = f"branchless.commit-metadata.{name}"
    try:
        return repo.config.get_bool(name)
    except KeyError:
        return default


class BranchesProvider:
    def __init__(self, glyphs: Glyphs, repo: pygit2.Repository) -> None:
        self._is_enabled = _is_enabled(repo=repo, name="branches", default=True)
        self._glyphs = glyphs
        oid_to_branches = collections.defaultdict(list)
        for branch_name in repo.listall_branches(pygit2.GIT_BRANCH_LOCAL):
            branch = repo.branches[branch_name]
            oid_to_branches[branch.resolve().target.hex].append(branch)
        self._oid_to_branches = oid_to_branches

    def __call__(self, oid: OidStr) -> Optional[str]:
        if not self._is_enabled:
            return None

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

    def __init__(self, glyphs: Glyphs, repo: pygit2.Repository) -> None:
        self._is_enabled = _is_enabled(
            repo=repo, name="differential-revision", default=True
        )
        self._glyphs = glyphs
        self._repo = repo

    def __call__(self, oid: OidStr) -> Optional[str]:
        if not self._is_enabled:
            return None

        commit_message = self._repo[oid].message
        match = self.RE.search(commit_message)
        if match is not None:
            revision_number = match.group("diff")
            return self._glyphs.color_fg(
                color=colorama.Fore.GREEN, message=revision_number
            )
        else:
            return None


class RelativeTimeProvider:
    def __init__(self, glyphs: Glyphs, repo: pygit2.Repository, now: int) -> None:
        self._is_enabled = _is_enabled(repo=repo, name="relative-time", default=True)
        self._glyphs = glyphs
        self._repo = repo
        self._now = now

    @staticmethod
    def _describe_time_delta(now: int, commit_time: int) -> str:
        time_delta = now - commit_time
        if time_delta < 60:
            return f"{time_delta}s"
        time_delta //= 60
        if time_delta < 60:
            return f"{time_delta}m"
        time_delta //= 60
        if time_delta < 24:
            return f"{time_delta}h"
        time_delta //= 24
        if time_delta < 365:
            return f"{time_delta}d"
        time_delta //= 365

        # Arguably at this point, users would want a specific date rather than a delta.
        return f"{time_delta}y"

    def __call__(self, oid: OidStr) -> Optional[str]:
        if not self._is_enabled:
            return None

        commit = self._repo[oid]
        description = self._describe_time_delta(
            now=self._now, commit_time=commit.commit_time
        )
        return self._glyphs.color_fg(
            color=colorama.Fore.GREEN,
            message=description,
        )
