"""Additional metadata to display for commits.

These are rendered inline in the smartlog, between the commit hash and the
commit message.
"""
import re
from typing import Callable, Dict, List, Optional

import colorama
import pygit2

from . import OidStr
from .formatting import Glyphs

CommitMetadataProvider = Callable[[pygit2.Commit], Optional[str]]
"""Interface to display information about a commit in the smartlog."""


def render_commit_metadata(
    glyphs: Glyphs,
    commit_metadata_providers: List[CommitMetadataProvider],
    commit: pygit2.Commit,
) -> str:
    """Get the complete description for a given commit.

    Args:
      glyphs: The glyphs to use.
      commit_metadata_providers: The providers of the metadata for the
        commit. These are displayed in order and concatenated with spaces.
      commit: The commit to render the metadata for.

    Returns:
      A string of additional metadata describing the commit.
    """
    metadata_list: List[Optional[str]] = [
        provider(commit) for provider in commit_metadata_providers
    ]
    return " ".join(text for text in metadata_list if text is not None)


def _is_enabled(repo: pygit2.Repository, name: str, default: bool) -> bool:
    name = f"branchless.commitMetadata.{name}"
    try:
        return repo.config.get_bool(name)
    except KeyError:
        return default


class CommitOidProvider:
    """Display an abbreviated commit hash."""

    def __init__(self, glyphs: Glyphs, use_color: bool) -> None:
        self._glyphs = glyphs
        self._use_color = use_color

    def __call__(self, commit: pygit2.Commit) -> Optional[str]:
        abbreviated_oid = f"{commit.oid!s:8.8}"
        if self._use_color:
            return self._glyphs.color_fg(
                color=colorama.Fore.YELLOW, message=abbreviated_oid
            )
        else:
            return abbreviated_oid


class CommitMessageProvider:
    """Display the first line of the commit message."""

    def __call__(self, commit: pygit2.Commit) -> Optional[str]:
        return commit.message.split("\n", 1)[0]


class BranchesProvider:
    """Display branches that point to a given commit."""

    def __init__(
        self,
        glyphs: Glyphs,
        repo: pygit2.Repository,
        branch_oid_to_names: Dict[OidStr, List[str]],
    ) -> None:
        self._is_enabled = _is_enabled(repo=repo, name="branches", default=True)
        self._glyphs = glyphs
        self._branch_oid_to_names = branch_oid_to_names

    def __call__(self, commit: pygit2.Commit) -> Optional[str]:
        if not self._is_enabled:
            return None

        branches = self._branch_oid_to_names.get(commit.oid.hex)
        if branches is not None:
            return self._glyphs.color_fg(
                color=colorama.Fore.GREEN,
                message="("
                + ", ".join(sorted(branch_name for branch_name in branches))
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
            repo=repo, name="differentialRevision", default=True
        )
        self._glyphs = glyphs

    def __call__(self, commit: pygit2.Commit) -> Optional[str]:
        if not self._is_enabled:
            return None

        match = self.RE.search(commit.message)
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
        self._is_enabled = _is_enabled(repo=repo, name="relativeTime", default=True)
        self._glyphs = glyphs
        self._now = now

    @staticmethod
    def describe_time_delta(now: int, previous_time: int) -> str:
        time_delta = now - previous_time
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

    def __call__(self, commit: pygit2.Commit) -> Optional[str]:
        if not self._is_enabled:
            return None

        description = self.describe_time_delta(
            now=self._now, previous_time=commit.commit_time
        )
        return self._glyphs.color_fg(
            color=colorama.Fore.GREEN,
            message=description,
        )
