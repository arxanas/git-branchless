"""Persistent storage to cache merge-base queries.

A "merge-base" can be described as the common ancestor of two commits.
Merge-bases are calculated to determine

 1) Whether a commit is a branch off of master.
 2) How to order two commits topologically.

In a large repository, merge-base queries can be quite expensive when
comparing commits which are far away from each other. This can happen, for
example, whenever you do a `git pull` to update `master`, but you haven't yet
updated any of your lines of work. Your lines of work are now far away from
the current `master` commit, so the merge-base calculation may take a while.
It can also happen when simply checking out an old commit to examine it.
"""
import sqlite3
from typing import List, Tuple, Optional

import pygit2

from .db import make_cursor


class MergeBaseDb:
    """Cache for merge-base queries."""

    def __init__(self, conn: sqlite3.Connection) -> None:
        """Constructor.

        Args:
          conn: The database connection.
        """
        self._conn = conn
        self._init_tables()
        self._num_requests = 0
        self._num_cache_hits = 0

    def _init_tables(self) -> None:
        with make_cursor(self._conn) as cursor:
            cursor.execute(
                """
CREATE TABLE IF NOT EXISTS merge_base_oids (
    lhs_oid TEXT NOT NULL,
    rhs_oid TEXT NOT NULL,
    merge_base_oid TEXT,
    UNIQUE (lhs_oid, rhs_oid)
)
    """
            )

    def is_empty(self) -> bool:
        """Determine if there are any entries in the cache.

        Returns:
          Whether or not there are any entries in the cache.
        """
        with make_cursor(self._conn) as cursor:
            result = cursor.execute(
                """
SELECT COUNT(*) = 0
FROM merge_base_oids
"""
            )
            results: List[Tuple[int]] = result.fetchall()
            [(is_empty,)] = results
            return bool(is_empty)

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
        self._num_requests += 1

        [lhs_oid_hex, rhs_oid_hex] = sorted([lhs_oid.hex, rhs_oid.hex])

        cursor = self._conn.cursor()
        result = cursor.execute(
            """
SELECT merge_base_oid
FROM merge_base_oids
WHERE lhs_oid = :lhs_oid
  AND rhs_oid = :rhs_oid
""",
            {
                "lhs_oid": lhs_oid_hex,
                "rhs_oid": rhs_oid_hex,
            },
        )
        results: List[Tuple[Optional[str]]] = result.fetchall()

        if len(results) > 0:
            self._num_cache_hits += 1
            (merge_base_oid_hex,) = results[0]
            if merge_base_oid_hex is None:
                return None
            else:
                merge_base = repo[merge_base_oid_hex]
                return merge_base.oid

        merge_base_oid = repo.merge_base(lhs_oid, rhs_oid)
        if merge_base_oid is None:
            merge_base_oid_hex = None
        else:
            merge_base_oid_hex = merge_base_oid.hex

        with make_cursor(self._conn) as cursor:
            cursor.execute(
                """
INSERT INTO merge_base_oids VALUES (
    :lhs_oid,
    :rhs_oid,
    :merge_base_oid
)
    """,
                {
                    "lhs_oid": lhs_oid_hex,
                    "rhs_oid": rhs_oid_hex,
                    "merge_base_oid": merge_base_oid_hex,
                },
            )
        return merge_base_oid

    @property
    def num_requests(self) -> int:
        """The number of requests made to the cache so far.

        Only includes requests since the process instantiation.
        """
        return self._num_requests

    @property
    def num_cache_hits(self) -> int:
        """The number of cache hits for requests to the cache so far.

        Only includes requests since the process instantiation.
        """
        return self._num_cache_hits

    def get_cache_hit_rate(self) -> Optional[float]:
        """The cache hit rate for requests not cache so far.

        Only includes requests since the process instantiation.

        Returns:
          The ratio of hits to total requests, or `None` if no requests have
          been made so far.
        """
        if self._num_requests == 0:
            return None
        else:
            return self._num_cache_hits / self._num_requests
