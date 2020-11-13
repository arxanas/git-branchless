import sqlite3
from typing import List, Tuple, Optional

import pygit2

from .db import make_cursor


class MergeBaseDb:
    """Cache for merge-base queries."""

    def __init__(self, conn: sqlite3.Connection) -> None:
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
        return self._num_requests

    @property
    def num_cache_hits(self) -> int:
        return self._num_cache_hits

    def get_cache_hit_rate(self) -> Optional[float]:
        if self._num_requests == 0:
            return None
        else:
            return self._num_cache_hits / self._num_requests
