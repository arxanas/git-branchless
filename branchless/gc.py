"""Deal with Git's garbage collection mechanism.

Git treats a commit as unreachable if there are no references that point to
it or one of its descendants. However, the branchless workflow oftentimes
involves keeping such commits reachable until the user has explicitly hidden
them.

This module is responsible for adding extra references to Git, so that Git's
garbage collection doesn't collect commits which branchless thinks are still
visible.
"""
import pygit2

from .rust import py_gc as gc


def mark_commit_reachable(repo: pygit2.Repository, commit_oid: pygit2.Oid) -> None:
    """Mark a commit as reachable.

    Once marked as reachable, the commit won't be collected by Git's garbage
    collection mechanism until first garbage-collected by branchless itself.

    Args:
      repo: The Git repository.
      commit_oid: The commit OID to mark as reachable.
    """
    ref_name = f"refs/branchless/{commit_oid.hex}"
    assert pygit2.reference_is_valid_name(ref_name)
    repo.references.create(name=ref_name, target=commit_oid, force=True)


# For flake8.
_ = gc
