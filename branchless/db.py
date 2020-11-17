"""Interact with persistent storage."""
import contextlib
import os
import sqlite3
from typing import Iterator

import pygit2


def make_db_for_repo(repo: pygit2.Repository) -> sqlite3.Connection:
    """Open a connection to the database for the repo.

    One SQLite database is associated with each Git repo. Sub-commands can
    establish their own tables within the database as necessary. The database
    is created if it does not exist.

    Returns:
      The connection to the SQLite database for this Git repo.
    """
    branchless_dir = os.path.join(repo.path, "branchless")
    try:
        os.mkdir(branchless_dir)
    except FileExistsError:
        pass
    db_path = os.path.join(branchless_dir, "db.sqlite3")
    return sqlite3.connect(db_path)


@contextlib.contextmanager
def make_cursor(conn: sqlite3.Connection) -> Iterator[sqlite3.Cursor]:
    """Context manager to commit cursor queries.

    When the context manager exits, the transaction is committed, unless an
    exception was raised, in which case the transaction is aborted.

    Yields:
      The cursor object to use for the transaction.
    """
    try:
        yield conn.cursor()
    except Exception:
        conn.rollback()
        raise
    finally:
        conn.commit()
