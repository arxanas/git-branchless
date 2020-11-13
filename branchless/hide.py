import sqlite3
from typing import Set, TextIO, cast, List, Tuple

import pygit2

from . import get_repo
from .db import make_cursor, make_db_for_repo
from .formatting import Formatter


class HideDb:
    def __init__(self, conn: sqlite3.Connection) -> None:
        self._conn = conn
        self._init_tables()

    def _init_tables(self) -> None:
        self._conn.execute(
            """
CREATE TABLE IF NOT EXISTS hidden_oids (
    oid TEXT UNIQUE NOT NULL
)
"""
        )

    def add_hidden_oid(self, oid: pygit2.Oid) -> bool:
        """Add a new hidden OID to the database.

        Returns whether or not the insertion succeeded (i.e., returns `False`
        if the given OID was already in the database).
        """
        with make_cursor(self._conn) as cursor:
            result = cursor.execute(
                """
    INSERT OR IGNORE INTO hidden_oids VALUES (:oid)
    """,
                {"oid": oid.hex},
            )
            return cast(int, result.rowcount) > 0

    def get_hidden_oids(self) -> Set[str]:
        """Find all hidden OIDs."""
        cursor = self._conn.cursor()
        result = cursor.execute(
            """
SELECT oid
FROM hidden_oids
"""
        )
        results: List[Tuple[str]] = result.fetchall()
        return set(oid for (oid,) in results)


def hide(*, out: TextIO, hash: str) -> int:
    formatter = Formatter()
    repo = get_repo()
    hide_db = HideDb(make_db_for_repo(repo))

    try:
        oid = repo.revparse_single(hash).oid
    except KeyError:
        out.write(f"Commit not found: {hash}\n")
        return 1

    hide_succeeded = hide_db.add_hidden_oid(oid)
    out.write(formatter.format("Hid commit: {oid:oid}\n", oid=oid))
    if not hide_succeeded:
        out.write("(It was already hidden, so this operation had no effect.)\n")
    out.write(
        formatter.format(
            "To unhide this commit, run: git checkout {oid:oid}\n", oid=oid
        )
    )
    return 0
