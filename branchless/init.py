"""Install any hooks, aliases, etc. to set up Branchless in this repo."""
from typing import TextIO

import colorama
import pygit2

from . import get_repo, parse_git_version_output, run_git_silent
from .formatting import make_glyphs
from .rust import py_init


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
    py_init(out=out, git_executable=git_executable)
    _install_aliases(out=out, repo=repo, git_executable=git_executable)
    return 0
