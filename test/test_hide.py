import io

from branchless.hide import hide, unhide
from branchless.smartlog import smartlog
from helpers import Git, compare


def test_hide_commit(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.run("checkout", ["master"])
    git.detach_head()
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
@ fe65c1fe create test2.txt
""",
        )

    with io.StringIO() as out:
        assert hide(out=out, hashes=["62fc20d2"]) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
Hid commit: 62fc20d2 create test1.txt
To unhide this commit, run: git unhide 62fc20d2
""",
        )

    with io.StringIO() as out:
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 (master) create initial.txt
|
@ fe65c1fe create test2.txt
""",
        )


def test_hide_bad_commit(git: Git) -> None:
    git.init_repo()
    with io.StringIO() as out:
        assert hide(out=out, hashes=["abc123"]) == 1
        compare(
            actual=out.getvalue(),
            expected="""\
Commit not found: abc123
""",
        )


def test_hide_transitive(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.commit_file(name="test3", time=3)
    git.run("checkout", ["master"])

    with io.StringIO() as out:
        smartlog(out=out)
        compare(
            actual=out.getvalue(),
            expected="""\
@ f777ecc9 (master) create initial.txt
|
o 62fc20d2 create test1.txt
|
o 96d1c37a create test2.txt
|
o 70deb1e2 create test3.txt
""",
        )

    with io.StringIO() as out:
        assert hide(out=out, hashes=["96d1c37a"]) == 0

    with io.StringIO() as out:
        smartlog(out=out)
        compare(
            actual=out.getvalue(),
            expected="""\
@ f777ecc9 (master) create initial.txt
|
o 62fc20d2 create test1.txt
""",
        )


def test_hide_already_hidden_commit(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)

    with io.StringIO() as out:
        assert hide(out=out, hashes=["62fc20d2"]) == 0

    with io.StringIO() as out:
        assert hide(out=out, hashes=["62fc20d2"]) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
Hid commit: 62fc20d2 create test1.txt
(It was already hidden, so this operation had no effect.)
To unhide this commit, run: git unhide 62fc20d2
""",
        )


def test_hide_current_commit(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test", time=1)

    with io.StringIO() as out:
        assert hide(out=out, hashes=["HEAD"]) == 0

    with io.StringIO() as out:
        assert smartlog(out=out) == 0
        compare(
            expected=out.getvalue(),
            actual="""\
O f777ecc9 (master) create initial.txt
|
% 3df4b935 create test.txt
""",
        )


def test_hidden_commit_with_head_as_child(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.commit_file(name="test3", time=3)
    git.run("checkout", ["HEAD^"])

    with io.StringIO() as out:
        assert hide(out=out, hashes=["HEAD^"]) == 0
        assert hide(out=out, hashes=["70deb1e2"]) == 0

    with io.StringIO() as out:
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 (master) create initial.txt
|
x 62fc20d2 create test1.txt
|
@ 96d1c37a create test2.txt
""",
        )


def test_hide_master_commit_with_hidden_children(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.detach_head()
    git.commit_file(name="test3", time=3)
    git.run("checkout", ["master"])
    git.commit_file(name="test4", time=4)
    git.commit_file(name="test5", time=5)
    git.run("reflog", ["delete", "HEAD@{1}"])

    with io.StringIO() as out:
        assert hide(out=out, hashes=["70deb1e2"]) == 0

    with io.StringIO() as out:
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
:
@ 20230db7 (master) create test5.txt
""",
        )


def test_branches_always_visible(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.run("branch", ["test"])
    git.run("checkout", ["master"])

    with io.StringIO() as out:
        assert hide(out=out, hashes=["test", "test^"]) == 0

    with io.StringIO() as out:
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
@ f777ecc9 (master) create initial.txt
|
x 62fc20d2 create test1.txt
|
x 96d1c37a (test) create test2.txt
""",
        )

    git.run("branch", ["-D", "test"])

    with io.StringIO() as out:
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
@ f777ecc9 (master) create initial.txt
""",
        )


def test_unhide(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.run("checkout", ["master"])

    with io.StringIO() as out:
        assert unhide(out=out, hashes=["96d1c37"]) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
Unhid commit: 96d1c37a create test2.txt
(It was not hidden, so this operation had no effect.)
To hide this commit, run: git hide 96d1c37a
""",
        )

    with io.StringIO() as out:
        assert hide(out=out, hashes=["96d1c37"]) == 0

    with io.StringIO() as out:
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
@ f777ecc9 (master) create initial.txt
|
o 62fc20d2 create test1.txt
""",
        )

    with io.StringIO() as out:
        assert unhide(out=out, hashes=["96d1c37"]) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
Unhid commit: 96d1c37a create test2.txt
To hide this commit, run: git hide 96d1c37a
""",
        )

    with io.StringIO() as out:
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
@ f777ecc9 (master) create initial.txt
|
o 62fc20d2 create test1.txt
|
o 96d1c37a create test2.txt
""",
        )
