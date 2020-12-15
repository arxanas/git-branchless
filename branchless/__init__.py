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

  * **Main**: A commit which has been checked into the main branch. No longer
    mutable. Visible to you in the branchless workflow.
  * **Visible**: A commit which you are working on currently. Visible to you in
    the branchless workflow.
  * **Hidden**: A commit which has been discarded or replaced. In particular,
    old versions of rebased commits are considered hidden. You can also
    manually hide commits that you no longer need. Not visible to you in the
    branchless workflow.
"""
import os
import subprocess
from typing import Dict, List, Set, TextIO, Tuple

import pygit2

OidStr = str
"""Represents an object ID in the Git repository.

We don't use `pygit2.Oid` directly since it requires looking up the object in
the repo, and we don't want to spend time hitting disk for that.
Consequently, the object pointed to by an OID is not guaranteed to exist
anymore (such as if it was garbage collected).
"""


def get_repo() -> pygit2.Repository:
    """Get the git repository associated with the current directory.

    Returns:
      The repository object associated with the current directory.

    Raises:
      RuntimeError: If the repository could not be found.
    """
    repo_path = pygit2.discover_repository(os.getcwd())
    if repo_path is None:
        raise RuntimeError("Failed to discover repository")
    return pygit2.Repository(repo_path)


def run_git(out: TextIO, err: TextIO, args: List[str]) -> int:
    """Run Git in a subprocess, and inform the user.

    This is suitable for commands which affect the working copy or should run
    hooks. We don't want our process to be responsible for that.

    Args:
      out: The output stream to write to.
      args: The list of arguments to pass to Git. Should not include the Git
        executable itself.

    Returns:
      The exit code of Git (non-zero signifies error).
    """
    args = ["git", *args]
    out.write(f"branchless: {' '.join(args)}\n")
    out.flush()
    err.flush()

    result = subprocess.run(args, stdout=out, stderr=err)
    return result.returncode


def run_git_silent(repo: pygit2.Repository, args: List[str]) -> str:
    """Run Git silently (don't display output to the user).

    Whenever possible, `pygit2`'s bindings to Git used instead, as they're
    considerably more lightweight and reliable.

    Args:
      repo: The Git repository.
      args: The command-line args to pass to Git. The `git` executable will
        be prepended to this list automatically.

    Raises:
      subprocess.CalledProcessError: if the command failed.

    Result:
      The output from the command.
    """
    result = subprocess.run(
        ["git", "-C", repo.path, *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=True,
    )
    return result.stdout.decode()


GitVersion = Tuple[int, int, int]
"""Version string produced by Git.

This tuple is in the form (major, minor, patch). You can do a version test by
using `<` on another version tuple.
"""


def parse_git_version_output(output: str) -> GitVersion:
    """Parse the `git version` output.

    Args:
      output: The output returned by `git version`.

    Returns:
      The parsed Git version.
    """
    [_git, _version, version_str, *_rest] = output.split(" ")
    [major, minor, patch, *_rest] = version_str.split(".")
    return (int(major), int(minor), int(patch))


def get_head_oid(repo: pygit2.Repository) -> pygit2.Oid:
    """Get the OID for the repository's `HEAD` reference.

    Args:
      repo: The Git repository.

    Returns:
      The OID for the repository's `HEAD` reference.
    """
    # We don't use `repo.head`, because that resolves the HEAD reference
    # (e.g. into refs/head/master). We want the actual ref-log of HEAD, not
    # the reference it points to.
    head_ref = repo.references["HEAD"]
    return head_ref.resolve().target


def get_main_branch_name(repo: pygit2.Repository) -> str:
    """Get the name of the main branch for the repository.

    Args:
      repo: The Git repository.

    Returns:
      The name of the main branch for the repository.
    """
    try:
        return repo.config["branchless.mainBranch"]
    except KeyError:
        return "master"


def get_main_branch_oid(repo: pygit2.Repository) -> pygit2.Oid:
    """Get the OID corresponding to the main branch.

    Args:
      repo: The Git repository.

    Raises:
      KeyError: if there was no such branch.

    Returns:
      The OID corresponding to the main branch.
    """
    main_branch_name = get_main_branch_name(repo)
    return repo.branches[main_branch_name].target


def get_branch_names(repo: pygit2.Repository) -> Set[str]:
    """Get the branch names for the repository.

    This only includes local branches; we don't want to spend time processing
    all of the remote branches, as they're unlikely to relate to the user's
    work.

    Args:
      repo: The Git repository.

    Returns:
      The set of branch names for the repository.
    """
    return set(repo.listall_branches(pygit2.GIT_BRANCH_LOCAL))


def get_branch_oid_to_names(repo: pygit2.Repository) -> Dict[OidStr, Set[str]]:
    """Get the mapping of branch OIDs to branch names.

    This mapping the lets you quickly look up which branches are pointing to
    a given OID, or just to enumerate OIDs/branch names in general.

    Args:
      repo: The Git repository.

    Returns:
      A mapping from an OID to the names of branches pointing to that
      OID.
    """
    result: Dict[OidStr, Set[str]] = {}
    branch_names = get_branch_names(repo)
    branch_names.add(get_main_branch_name(repo))
    for branch_name in branch_names:
        branch_oid = repo.branches[branch_name].target.hex
        if branch_oid not in result:
            result[branch_oid] = set()
        result[branch_oid].add(branch_name)
    return result
