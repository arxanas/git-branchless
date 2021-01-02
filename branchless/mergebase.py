"""Persistent storage to cache merge-base queries.

A "merge-base" can be described as the common ancestor of two commits.
Merge-bases are calculated to determine

 1) Whether a commit is a branch off of the main branch.
 2) How to order two commits topologically.

In a large repository, merge-base queries can be quite expensive when
comparing commits which are far away from each other. This can happen, for
example, whenever you do a `git pull` to update the main branch, but you
haven't yet updated any of your lines of work. Your lines of work are now far
away from the current main branch commit, so the merge-base calculation may
take a while. It can also happen when simply checking out an old commit to
examine it.
"""
from .rust import PyMergeBaseDb

MergeBaseDb = PyMergeBaseDb
