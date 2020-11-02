import io
import subprocess
import sys
from typing import Any, List, Optional

import py
import pytest

from branchless import main

GIT_PATH = "/opt/twitter_mde/bin/git"

DUMMY_NAME = "Testy McTestface"
DUMMY_EMAIL = "test@example.com"
DUMMY_DATE = "Wed 29 Oct 12:34:56 2020 PDT"


def git(command: str, args: Optional[List[str]] = None) -> str:
    if args is None:
        args = []
    args = [GIT_PATH, command, *args]

    # Required for determinism, as these values will be baked into the commit
    # hash.
    env = {
        "GIT_AUTHOR_NAME": DUMMY_NAME,
        "GIT_AUTHOR_EMAIL": DUMMY_EMAIL,
        "GIT_AUTHOR_DATE": DUMMY_DATE,
        "GIT_COMMITTER_NAME": DUMMY_NAME,
        "GIT_COMMITTER_EMAIL": DUMMY_EMAIL,
        "GIT_COMMITTER_DATE": DUMMY_DATE,
    }

    result = subprocess.run(args, stdout=subprocess.PIPE, env=env)
    return result.stdout.decode()


def git_commit_file(cwd: py.path.local, name: str) -> None:
    cwd.join(f"{name}.txt").write(f"{name} contents\n")
    git("add", ["."])
    git("commit", ["-m", f"create {name}.txt"])


def git_initial_commit(cwd: py.path.local) -> None:
    git("init")
    git_commit_file(cwd, name="initial")


def test_help() -> None:
    with io.StringIO() as out:
        main(["--help"], out=out)
        assert "usage: branchless" in out.getvalue()


def test_init(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit(tmpdir)

        main(["smartlog"], out=out)
        assert (
            out.getvalue()
            == """\
c967edc2 create initial.txt
"""
        )


def test_show_reachable_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit(tmpdir)
        git("checkout", ["-b", "initial-branch"])
        git_commit_file(tmpdir, name="test")

        main(["smartlog"], out=out)
        assert (
            out.getvalue()
            == """\
e5aad67e create test.txt
"""
        )
