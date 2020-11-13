import io

import py
from branchless.smartlog import smartlog

from helpers import (
    compare,
    git,
    git_commit_file,
    git_detach_head,
    git_initial_commit,
    git_resolve_file,
)


def test_init(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()

        assert smartlog(out=out, show_old_commits=True) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
* f777ecc9 create initial.txt
""",
        )


def test_show_reachable_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()
        git("checkout", ["-b", "initial-branch", "master"])
        git_commit_file(name="test", time=1)

        assert smartlog(out=out, show_old_commits=True) == 0
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

        assert smartlog(out=out, show_old_commits=True) == 0
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
        git("checkout", ["-b", "test1", "master"])
        git_commit_file(name="test1", time=1)
        git("checkout", ["master"])
        git_detach_head()
        git_commit_file(name="test2", time=2)
        git("rebase", ["test1"])

        assert smartlog(out=out, show_old_commits=True) == 0
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

        assert smartlog(out=out, show_old_commits=True) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
* 70deb1e2 create test3.txt
:
""",
        )


def test_merge_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()
        git("checkout", ["-b", "test1", "master"])
        git_commit_file(name="test1", time=1)
        git("checkout", ["-b", "test2and3", "master"])
        git_commit_file(name="test2", time=2)
        git_commit_file(name="test3", time=3)
        git("merge", ["test1"], time=4)

        # Note that we may want to change the rendering/handling of merge
        # commits in the future. Currently, we ignore the fact that merge
        # commits have multiple parents, and pick a single parent with which to
        # render it. We also don't properly mark the merged-from commits as
        # hidden.
        assert smartlog(out=out, show_old_commits=True) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
o fe65c1fe create test2.txt
|
| * fa4e4e1a Merge branch 'test1' into test2and3
| |
| o 62fc20d2 create test1.txt
|/
o f777ecc9 create initial.txt
""",
        )


def test_rebase_conflict(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()
        git("checkout", ["-b", "branch1", "master"])
        git_commit_file(name="test", time=1, contents="contents 1")
        git("checkout", ["-b", "branch2", "master"])
        git_commit_file(name="test", time=2, contents="contents 2")

        # Should produce a conflict.
        git("rebase", ["branch1"])
        git_resolve_file(name="test", contents="contents resolved")
        git("rebase", ["--continue"])

        assert smartlog(out=out, show_old_commits=True) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
* 4549af33 create test.txt
|
o 88646b56 create test.txt
|
o f777ecc9 create initial.txt
""",
        )


def test_non_adjacent_commits(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git("checkout", ["master"])
        git_commit_file(name="test2", time=2)
        git_commit_file(name="test3", time=3)
        git_detach_head()
        git_commit_file(name="test4", time=4)

        assert smartlog(out=out, show_old_commits=True) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
* 8e62740b create test4.txt
|
o 02067177 create test3.txt
:
: o 62fc20d2 create test1.txt
|/
o f777ecc9 create initial.txt
""",
        )


def test_non_adjacent_commits2(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_initial_commit()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)
        git("checkout", ["master"])
        git_commit_file(name="test3", time=3)
        git_commit_file(name="test4", time=4)
        git_detach_head()
        git_commit_file(name="test5", time=5)

        assert smartlog(out=out, show_old_commits=True) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
* 13932989 create test5.txt
|
o 2b633ed7 create test4.txt
:
: o 96d1c37a create test2.txt
: |
: o 62fc20d2 create test1.txt
|/
o f777ecc9 create initial.txt
""",
        )
