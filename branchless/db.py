import contextlib
import os
import sqlite3
from typing import Iterator

import pygit2


def make_db_for_repo(repo: pygit2.Repository) -> sqlite3.Connection:
    branchless_dir = os.path.join(repo.path, "branchless")
    try:
        os.mkdir(branchless_dir)
    except FileExistsError:
        pass
    db_path = os.path.join(branchless_dir, "db.sqlite3")
    return sqlite3.connect(db_path)


@contextlib.contextmanager
def make_cursor(conn: sqlite3.Connection) -> Iterator[sqlite3.Cursor]:
    try:
        yield conn.cursor()
    except Exception:
        conn.rollback()
    finally:
        conn.commit()
