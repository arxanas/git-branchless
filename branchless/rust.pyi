import sqlite3
from typing import Optional

import pygit2

class PyMergeBaseDb:
    """Cache for merge-base queries."""

    def __init__(self, conn: sqlite3.Connection) -> None:
        """Constructor.

        Args:
          conn: The database connection.
        """
        ...
    def get_merge_base_oid(
        self, repo: pygit2.Repository, lhs_oid: pygit2.Oid, rhs_oid: pygit2.Oid
    ) -> Optional[pygit2.Oid]:
        """Get the merge-base for two given commits.

        If the query is already in the cache, return the cached result. If
        not, it is computed, cached, and returned.

        Args:
          repo: The Git repo.
          lhs_oid: The first OID (ordering is arbitrary).
          rhs_oid: The second OID (ordering is arbitrary).

        Returns:
          The merge-base OID for these two commits. Returns `None` if no
          merge-base could be found.
        """
        ...
