"""Install any hooks, aliases, etc. to set up Branchless in this repo."""
import logging
import stat
from dataclasses import dataclass
from pathlib import Path
from typing import List, TextIO, Union

import colorama
import pygit2

from . import get_repo, parse_git_version_output, run_git_silent
from .formatting import make_glyphs


@dataclass(frozen=True, eq=True)
class _RegularHook:
    path: Path


@dataclass(frozen=True, eq=True)
class _MultiHook:
    path: Path


_Hook = Union[_RegularHook, _MultiHook]


def _determine_hook_path(repo: pygit2.Repository, hook_type: str) -> _Hook:
    """Determine the path for the hook to the installed.

    Handles the multi-hook system in use at Twitter.
    """
    multi_hooks_path = Path(repo.path).joinpath() / "hooks_multi"
    if multi_hooks_path.exists():
        return _MultiHook(
            path=multi_hooks_path / (hook_type + ".d") / "00_local_branchless"
        )

    try:
        hooks_dir = Path(repo.config["core.hooksPath"])
    except KeyError:
        hooks_dir = Path(repo.path) / "hooks"
    return _RegularHook(path=hooks_dir / hook_type)


_SHEBANG = "#!/bin/sh\n"

_UPDATE_MARKER_START = "## START BRANCHLESS CONFIG\n"

_UPDATE_MARKER_END = "## END BRANCHLESS CONFIG\n"


def _update_between_lines(lines: List[str], updated_lines: List[str]) -> List[str]:
    new_lines = []
    is_ignoring_lines = False
    for line in lines:
        if line == _UPDATE_MARKER_START:
            is_ignoring_lines = True
            new_lines.append(_UPDATE_MARKER_START)
            new_lines.extend(updated_lines)
            new_lines.append(_UPDATE_MARKER_END)
        elif line == _UPDATE_MARKER_END:
            is_ignoring_lines = False
        elif not is_ignoring_lines:
            new_lines.append(line)
    if is_ignoring_lines:
        logging.warning("Unterminated branchless config comment in hook")
    return new_lines


def _update_hook_contents(hook: _Hook, hook_contents: str) -> None:
    """Update the given hook script."""
    if isinstance(hook, _RegularHook):
        try:
            lines = hook.path.read_text().splitlines(keepends=True)
            lines = _update_between_lines(
                lines, hook_contents.splitlines(keepends=True)
            )
            hook_contents = "".join(lines)
        except FileNotFoundError:
            hook_contents = (
                _SHEBANG + _UPDATE_MARKER_START + hook_contents + _UPDATE_MARKER_END
            )
    elif isinstance(hook, _MultiHook):
        # Can safely overwrite, since our hook exists in its own file. No need
        # to update the hook contents.
        pass
    else:  # pragma: no cover
        raise TypeError(f"Unknown hook type: {hook}")

    hook.path.parent.mkdir(parents=True, exist_ok=True)
    hook.path.write_text(hook_contents)

    # Mark hook as executable.
    hook.path.chmod(
        hook.path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
    )


def _install_hook(
    out: TextIO, repo: pygit2.Repository, hook_type: str, hook_script: str
) -> None:
    out.write(f"Installing hook: {hook_type}\n")
    hook = _determine_hook_path(repo=repo, hook_type=hook_type)
    _update_hook_contents(hook, hook_script)


def _install_hooks(out: TextIO, repo: pygit2.Repository, git_executable: str) -> None:
    _install_hook(
        out=out,
        repo=repo,
        hook_type="post-commit",
        hook_script="""\
git branchless hook-post-commit "$@"
""",
    )
    _install_hook(
        out=out,
        repo=repo,
        hook_type="post-rewrite",
        hook_script="""\
git branchless hook-post-rewrite "$@"
""",
    )
    _install_hook(
        out=out,
        repo=repo,
        hook_type="post-checkout",
        hook_script="""\
git branchless hook-post-checkout "$@"
""",
    )
    _install_hook(
        out=out,
        repo=repo,
        hook_type="pre-auto-gc",
        hook_script="""\
git branchless hook-pre-auto-gc "$@"
""",
    )
    _install_hook(
        out=out,
        repo=repo,
        hook_type="reference-transaction",
        hook_script="""\
# Avoid canceling the reference transaction in the case that `branchless` fails
# for whatever reason.
git branchless hook-reference-transaction "$@" || (
    echo 'branchless: Failed to process reference transaction!'
    echo 'branchless: Some events (e.g. branch updates) may have been lost.'
    echo 'branchless: This is a bug. Please report it.'
)
""",
    )


def _install_alias(out: TextIO, repo: pygit2.Repository, alias: str) -> None:
    out.write(f"Installing alias (non-global): git {alias}\n")
    repo.config[f"alias.{alias}"] = f"branchless {alias}"


def _install_aliases(out: TextIO, repo: pygit2.Repository, git_executable: str) -> None:
    _install_alias(out=out, repo=repo, alias="smartlog")
    _install_alias(out=out, repo=repo, alias="sl")
    _install_alias(out=out, repo=repo, alias="hide")
    _install_alias(out=out, repo=repo, alias="unhide")
    _install_alias(out=out, repo=repo, alias="prev")
    _install_alias(out=out, repo=repo, alias="next")
    _install_alias(out=out, repo=repo, alias="restack")
    _install_alias(out=out, repo=repo, alias="undo")

    version_str = run_git_silent(
        repo=repo, git_executable=git_executable, args=["version"]
    ).strip()
    version = parse_git_version_output(version_str)
    if version < (2, 29, 0):
        glyphs = make_glyphs(out)
        warning_str = glyphs.style(
            style=colorama.Style.BRIGHT,
            message=glyphs.color_fg(color=colorama.Fore.YELLOW, message="Warning"),
        )
        out.write(
            f"""\
{warning_str}: the branchless workflow's "git undo" command requires Git
v2.29 or later, but your Git version is: {version_str}

Some operations, such as branch updates, won't be correctly undone. Other
operations may be undoable. Attempt at your own risk.

Once you upgrade to Git v2.9, run `git branchless init` again. Any work you
do from then on will be correctly undoable.

This only applies to the "git undo" command. Other commands which are part of
the branchless workflow will work properly.
"""
        )


def init(*, out: TextIO, git_executable: str) -> int:
    """Initialize Branchless in the current repo.

    Args:
      out: The output stream to write to.
      git_executable: The path to the `git` executable on disk.

    Returns:
      Exit code (0 denotes successful exit).
    """
    repo = get_repo()
    _install_hooks(out=out, repo=repo, git_executable=git_executable)
    _install_aliases(out=out, repo=repo, git_executable=git_executable)
    return 0
