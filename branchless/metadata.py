"""Additional metadata to display for commits.

These are rendered inline in the smartlog, between the commit hash and the
commit message.
"""
import collections
import re
from typing import Callable, List, Optional

import colorama
import pygit2

from .formatting import Formatter, Glyphs
from .graph import Node

CommitMetadataProvider = Callable[[Node], Optional[str]]
"""Interface to display information about a commit in the smartlog."""


def get_commit_metadata(
    formatter: Formatter,
    glyphs: Glyphs,
    commit_metadata_providers: List[CommitMetadataProvider],
    node: Node,
) -> Optional[str]:
    """Get the complete description for a given commit.

    Args:
      formatter: The formatter to use.
      glyphs: The glyphs to use.
      commit_metadata_providers: The providers of the metadata for the
        commit. These are displayed in order and concatenated with spaces.
      node: The node representing the commit to describe.

    Returns:
      A string of additional metadata describing the commit.
    """
    metadata_list: List[Optional[str]] = [
        provider(node) for provider in commit_metadata_providers
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
    """Display branches that point to a given commit."""

    def __init__(self, glyphs: Glyphs, repo: pygit2.Repository) -> None:
        self._is_enabled = _is_enabled(repo=repo, name="branches", default=True)
        self._glyphs = glyphs
        oid_to_branches = collections.defaultdict(list)
        for branch_name in repo.listall_branches(pygit2.GIT_BRANCH_LOCAL):
            branch = repo.branches[branch_name]
            oid_to_branches[branch.resolve().target.hex].append(branch)
        self._oid_to_branches = oid_to_branches

    def __call__(self, node: Node) -> Optional[str]:
        if not self._is_enabled:
            return None

        branches = self._oid_to_branches[node.commit.oid.hex]
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
    """Display the associated Phabricator revision for a given commit."""

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

    def __call__(self, node: Node) -> Optional[str]:
        if not self._is_enabled:
            return None

        match = self.RE.search(node.commit.message)
        if match is not None:
            revision_number = match.group("diff")
            return self._glyphs.color_fg(
                color=colorama.Fore.GREEN, message=revision_number
            )
        else:
            return None


class RelativeTimeProvider:
    """Display how long ago the given commit was committed."""

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

    def __call__(self, node: Node) -> Optional[str]:
        if not self._is_enabled:
            return None

        commit = node.commit
        description = self._describe_time_delta(
            now=self._now, commit_time=commit.commit_time
        )
        return self._glyphs.color_fg(
            color=colorama.Fore.GREEN,
            message=description,
        )
