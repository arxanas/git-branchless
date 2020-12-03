import io

from branchless.smartlog import smartlog
from helpers import Git, compare


def test_init(git: Git) -> None:
    git.init_repo()
    with io.StringIO() as out:
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
@ f777ecc9 (master) create initial.txt
""",
        )


def test_show_reachable_commit(git: Git) -> None:
    git.init_repo()
    git.run("checkout", ["-b", "initial-branch", "master"])
    git.commit_file(name="test", time=1)

    with io.StringIO() as out:
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 (master) create initial.txt
|
@ 3df4b935 (initial-branch) create test.txt
""",
        )


def test_tree(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.run("branch", ["initial"])
    git.commit_file(name="test1", time=1)
    git.run("checkout", ["initial"])
    git.commit_file(name="test2", time=2)

    with io.StringIO() as out:
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


def test_rebase(git: Git) -> None:
    git.init_repo()
    git.run("checkout", ["-b", "test1", "master"])
    git.commit_file(name="test1", time=1)
    git.run("checkout", ["master"])
    git.detach_head()
    git.commit_file(name="test2", time=2)
    git.run("rebase", ["test1"])

    with io.StringIO() as out:
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


def test_sequential_master_commits(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.commit_file(name="test3", time=3)

    with io.StringIO() as out:
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
:
@ 70deb1e2 (master) create test3.txt
""",
        )


def test_merge_commit(git: Git) -> None:
    git.init_repo()
    git.run("checkout", ["-b", "test1", "master"])
    git.commit_file(name="test1", time=1)
    git.run("checkout", ["-b", "test2and3", "master"])
    git.commit_file(name="test2", time=2)
    git.commit_file(name="test3", time=3)
    git.run("merge", ["test1"], time=4)

    with io.StringIO() as out:
        # Rendering here is arbitrary and open to change.
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
|
o 02067177 create test3.txt
|
@ fa4e4e1a (test2and3) Merge branch 'test1' into test2and3
""",
        )


def test_rebase_conflict(git: Git) -> None:
    git.init_repo()
    git.run("checkout", ["-b", "branch1", "master"])
    git.commit_file(name="test", time=1, contents="contents 1")
    git.run("checkout", ["-b", "branch2", "master"])
    git.commit_file(name="test", time=2, contents="contents 2")

    # Should produce a conflict.
    git.run("rebase", ["branch1"], check=False)
    git.resolve_file(name="test", contents="contents resolved")
    git.run("rebase", ["--continue"])

    with io.StringIO() as out:
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


def test_non_adjacent_commits(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.run("checkout", ["master"])
    git.commit_file(name="test2", time=2)
    git.commit_file(name="test3", time=3)
    git.detach_head()
    git.commit_file(name="test4", time=4)

    with io.StringIO() as out:
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


def test_non_adjacent_commits2(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.run("checkout", ["master"])
    git.commit_file(name="test3", time=3)
    git.commit_file(name="test4", time=4)
    git.detach_head()
    git.commit_file(name="test5", time=5)

    with io.StringIO() as out:
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


def test_non_adjacent_commits3(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.detach_head()
    git.commit_file(name="test2", time=2)
    git.run("checkout", ["master"])
    git.commit_file(name="test3", time=3)
    git.detach_head()
    git.commit_file(name="test4", time=4)
    git.run("checkout", ["master"])
    git.commit_file(name="test5", time=5)
    git.commit_file(name="test6", time=6)

    with io.StringIO() as out:
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
