import io
import os
import subprocess
import sys
from typing import Any, List, Optional

import py
import pytest

from branchless.__main__ import main

GIT_PATH = "/opt/twitter_mde/bin/git"

DUMMY_NAME = "Testy McTestface"
DUMMY_EMAIL = "test@example.com"
DUMMY_DATE = "Wed 29 Oct 12:34:56 2020 PDT"


def git(command: str, args: Optional[List[str]] = None, time: int = 0) -> str:
    if args is None:
        args = []
    args = [GIT_PATH, command, *args]

    # Required for determinism, as these values will be baked into the commit
    # hash.
    date = f"{DUMMY_DATE} -{time:02d}00"
    env = {
        "GIT_AUTHOR_NAME": DUMMY_NAME,
        "GIT_AUTHOR_EMAIL": DUMMY_EMAIL,
        "GIT_AUTHOR_DATE": date,
        "GIT_COMMITTER_NAME": DUMMY_NAME,
        "GIT_COMMITTER_EMAIL": DUMMY_EMAIL,
        "GIT_COMMITTER_DATE": date,
    }

    result = subprocess.run(args, stdout=subprocess.PIPE, env=env)
    return result.stdout.decode()


def git_commit_file(name: str, time: int) -> None:
    path = os.path.join(os.getcwd(), f"{name}.txt")
    with open(path, "w") as f:
        f.write(f"{name} contents\n")
    git("add", ["."])
    git("commit", ["-m", f"create {name}.txt"], time=time)


def git_initial_commit() -> None:
    git("init")
    git_commit_file(name="initial", time=0)


def detach_head() -> None:
    git("checkout", ["--detach", "HEAD"])


def rstrip_lines(lines: str) -> str:
    return "".join(line.rstrip() + "\n" for line in lines.splitlines())


def compare(actual: str, expected: str) -> None:
    actual = rstrip_lines(actual)
    expected = rstrip_lines(expected)
    print("Expected:")
    print(expected)
    print("Actual:")
    print(actual)
    assert actual == expected


def test_help() -> None:
    with io.StringIO() as out:
        main(["--help"], out=out)
        assert "usage: branchless" in out.getvalue()


def test_init(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()

        main(["smartlog"], out=out)
        compare(
            actual=out.getvalue(),
            expected="""\
o f777ecc9 create initial.txt
""",
        )


def test_show_reachable_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()
        git("checkout", ["-b", "initial-branch"])
        git_commit_file(name="test", time=1)

        main(["smartlog"], out=out)
        compare(
            actual=out.getvalue(),
            expected="""\
o 3df4b935 create test.txt
| 
o f777ecc9 create initial.txt
""",
        )


def test_tree(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()
        detach_head()
        git("branch", ["initial"])
        git_commit_file(name="test1", time=1)
        git("checkout", ["initial"])
        git_commit_file(name="test2", time=2)

        main(["smartlog"], out=out)
        compare(
            actual=out.getvalue(),
            expected="""\
o fe65c1fe create test2.txt
|
| o 62fc20d2 create test1.txt
|/
o f777ecc9 create initial.txt
""",
        )
