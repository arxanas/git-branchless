import io

import py
from branchless.smartlog import smartlog

from helpers import (
    compare,
    git,
    git_commit_file,
    git_detach_head,
    git_init_repo,
    git_resolve_file,
)


def test_init(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_init_repo()

        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
@ f777ecc9 (master) create initial.txt
""",
        )


def test_show_reachable_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_init_repo()
        git("checkout", ["-b", "initial-branch", "master"])
        git_commit_file(name="test", time=1)

        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 (master) create initial.txt
|
@ 3df4b935 (initial-branch) create test.txt
""",
        )


def test_tree(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_init_repo()
        git_detach_head()
        git("branch", ["initial"])
        git_commit_file(name="test1", time=1)
        git("checkout", ["initial"])
        git_commit_file(name="test2", time=2)

        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 (master) create initial.txt
|\\
| o 62fc20d2 create test1.txt
|
@ fe65c1fe (initial) create test2.txt
""",
        )


def test_rebase(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_init_repo()
        git("checkout", ["-b", "test1", "master"])
        git_commit_file(name="test1", time=1)
        git("checkout", ["master"])
        git_detach_head()
        git_commit_file(name="test2", time=2)
        git("rebase", ["test1"])

        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 (master) create initial.txt
|
o 62fc20d2 (test1) create test1.txt
|
@ f8d9985b create test2.txt
""",
        )


def test_sequential_master_commits(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_init_repo()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)
        git_commit_file(name="test3", time=3)

        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
:
@ 70deb1e2 (master) create test3.txt
""",
        )


def test_merge_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_init_repo()
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
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 (master) create initial.txt
|\\
| o 62fc20d2 (test1) create test1.txt
| |
| @ fa4e4e1a (test2and3) Merge branch 'test1' into test2and3
|
o fe65c1fe create test2.txt
""",
        )


def test_rebase_conflict(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_init_repo()
        git("checkout", ["-b", "branch1", "master"])
        git_commit_file(name="test", time=1, contents="contents 1")
        git("checkout", ["-b", "branch2", "master"])
        git_commit_file(name="test", time=2, contents="contents 2")

        # Should produce a conflict.
        git("rebase", ["branch1"], check=False)
        git_resolve_file(name="test", contents="contents resolved")
        git("rebase", ["--continue"])

        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 (master) create initial.txt
|
o 88646b56 (branch1) create test.txt
|
@ 4549af33 (branch2) create test.txt
""",
        )


def test_non_adjacent_commits(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git("checkout", ["master"])
        git_commit_file(name="test2", time=2)
        git_commit_file(name="test3", time=3)
        git_detach_head()
        git_commit_file(name="test4", time=4)

        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 create initial.txt
|\\
: o 62fc20d2 create test1.txt
:
O 02067177 (master) create test3.txt
|
@ 8e62740b create test4.txt
""",
        )


def test_non_adjacent_commits2(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)
        git("checkout", ["master"])
        git_commit_file(name="test3", time=3)
        git_commit_file(name="test4", time=4)
        git_detach_head()
        git_commit_file(name="test5", time=5)

        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 create initial.txt
|\\
: o 62fc20d2 create test1.txt
: |
: o 96d1c37a create test2.txt
:
O 2b633ed7 (master) create test4.txt
|
@ 13932989 create test5.txt
""",
        )


def test_non_adjacent_commits3(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_init_repo()
        git_commit_file(name="test1", time=1)
        git_detach_head()
        git_commit_file(name="test2", time=2)
        git("checkout", ["master"])
        git_commit_file(name="test3", time=3)
        git_detach_head()
        git_commit_file(name="test4", time=4)
        git("checkout", ["master"])
        git_commit_file(name="test5", time=5)
        git_commit_file(name="test6", time=6)

        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
:
O 62fc20d2 create test1.txt
|\\
| o 96d1c37a create test2.txt
|
O 4838e49b create test3.txt
|\\
: o a2482074 create test4.txt
:
@ 500c9b3e (master) create test6.txt
""",
        )


def test_amended_initial_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_commit_file(name="test1", time=1)
        git("checkout", ["HEAD^"])
        git("commit", ["--amend", "-m", "new initial commit"])

        with io.StringIO() as out:
            assert smartlog(out=out) == 0
            # Pathological output, could be changed.
            compare(
                actual=out.getvalue(),
                expected="""\
:
O 62fc20d2 (master) create test1.txt
""",
            )

        git("rebase", ["--onto", "HEAD", "HEAD", "master"])
        with io.StringIO() as out:
            assert smartlog(out=out) == 0
            compare(
                actual=out.getvalue(),
                expected="""\
:
@ f402d39c (master) create test1.txt
""",
            )
