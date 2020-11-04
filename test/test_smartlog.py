import io
import os
import subprocess
import sys
from typing import Any, List, Optional

import py
import pytest

from branchless.smartlog import smartlog
from helpers import git, git_initial_commit, git_commit_file, git_detach_head


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


def test_init(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()

        smartlog(out=out, show_old_commits=False)
        compare(
            actual=out.getvalue(),
            expected="""\
* f777ecc9 create initial.txt
""",
        )


def test_show_reachable_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()
        git("checkout", ["-b", "initial-branch"])
        git_commit_file(name="test", time=1)

        smartlog(out=out, show_old_commits=False)
        compare(
            actual=out.getvalue(),
            expected="""\
* 3df4b935 create test.txt
| 
o f777ecc9 create initial.txt
""",
        )


def test_tree(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()
        git_detach_head()
        git("branch", ["initial"])
        git_commit_file(name="test1", time=1)
        git("checkout", ["initial"])
        git_commit_file(name="test2", time=2)

        smartlog(out=out, show_old_commits=False)
        compare(
            actual=out.getvalue(),
            expected="""\
* fe65c1fe create test2.txt
|
| o 62fc20d2 create test1.txt
|/
o f777ecc9 create initial.txt
""",
        )


def test_rebase(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()
        git("checkout", ["-b", "test1"])
        git_commit_file(name="test1", time=1)
        git("checkout", ["HEAD^"])
        git_commit_file(name="test2", time=2)
        git("rebase", ["test1"])

        smartlog(out=out, show_old_commits=False)
        compare(
            actual=out.getvalue(),
            expected="""\
* f8d9985b create test2.txt
|
o 62fc20d2 create test1.txt
|
o f777ecc9 create initial.txt
""",
        )


def test_sequential_master_commits(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)
        git_commit_file(name="test3", time=3)

        smartlog(out=out, show_old_commits=False)
        compare(
            actual=out.getvalue(),
            expected="""\
* 70deb1e2 create test3.txt
|
o 96d1c37a create test2.txt
|
o 62fc20d2 create test1.txt
|
o f777ecc9 create initial.txt
""",
        )