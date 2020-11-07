"""Branchless workflow for Git.

# Why?

Most Git workflows involve heavy use of branches to track commit work that is
underway. However, branches require that you "name" every commit you're
interested in tracking. If you spend a lot of time doing any of the following:

  * Switching between work tasks.
  * Separating minor cleanups/refactorings into their own commits, for ease of
    reviewability.
  * Performing speculative work which may not be ultimately committed.
  * Working on top of work that you or a collaborator produced, which is not
    yet checked in.
  * Losing track of `git stash`es you made previously.

Then the branchless workflow may be for you instead.

# Branchless workflow and concepts

The branchless workflow does away with needing to explicitly name commits
with branches (although you are free to do so if you like). Rather than use
branches to see your current work items, you simply make commits as you go.

The branchless extensions infer which commits you're working on, and display
them to you with the `git smartlog` (or `git sl`) command.

A commit is in one of three states:

  * **Master**: A commit which has been checked into master. No longer mutable.
    Visible to you in the branchless workflow.
  * **Visible**: A commit which you are working on currently. Visible to you in
    the branchless workflow.
  * **Hidden**: A commit which has been discarded or replaced. In particular,
    old versions of rebased commits are considered hidden. You can also
    manually hide commits that you no longer need. Not visible to you in the
    branchless workflow.
"""
import os
from typing import Literal, Union

import pygit2

CommitStatus = Union[Literal["master"], Literal["visible"], Literal["hidden"]]


def get_repo() -> pygit2.Repository:
    """Get the git repository associated with the current directory."""
    repo_path = pygit2.discover_repository(os.getcwd())
    assert repo_path is not None, "Failed to discover repository"
    return pygit2.Repository(repo_path)
