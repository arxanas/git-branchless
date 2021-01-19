"""Deal with Git's garbage collection mechanism.

Git treats a commit as unreachable if there are no references that point to
it or one of its descendants. However, the branchless workflow oftentimes
involves keeping such commits reachable until the user has explicitly hidden
them.

This module is responsible for adding extra references to Git, so that Git's
garbage collection doesn't collect commits which branchless thinks are still
visible.
"""
from .rust import py_gc as gc
from .rust import py_mark_commit_reachable as mark_commit_reachable

# For flake8.
_ = (mark_commit_reachable, gc)
