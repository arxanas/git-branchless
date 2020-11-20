import io

import py

from branchless.hide import hide, unhide
from branchless.smartlog import smartlog
from helpers import compare, git, git_commit_file, git_detach_head, git_init_repo


def test_hide_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git("checkout", ["master"])
        git_detach_head()
        git_commit_file(name="test2", time=2)

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
Hid commit: 62fc20d2
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


def test_hide_bad_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_init_repo()
        assert hide(out=out, hashes=["abc123"]) == 1
        compare(
            actual=out.getvalue(),
            expected="""\
Commit not found: abc123
""",
        )


def test_hide_transitive(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)
        git_commit_file(name="test3", time=3)
        git("checkout", ["master"])

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


def test_hide_already_hidden_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)

        with io.StringIO() as out:
            assert hide(out=out, hashes=["62fc20d2"]) == 0

        with io.StringIO() as out:
            assert hide(out=out, hashes=["62fc20d2"]) == 0
            compare(
                actual=out.getvalue(),
                expected="""\
Hid commit: 62fc20d2
(It was already hidden, so this operation had no effect.)
To unhide this commit, run: git unhide 62fc20d2
""",
            )


def test_hide_current_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test", time=1)

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


def test_hidden_commit_with_head_as_child(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)
        git_commit_file(name="test3", time=3)
        git("checkout", ["HEAD^"])

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


def test_hide_master_commit_with_hidden_children(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)
        git_detach_head()
        git_commit_file(name="test3", time=3)
        git("checkout", ["master"])
        git_commit_file(name="test4", time=4)
        git_commit_file(name="test5", time=5)
        git("reflog", ["delete", "HEAD@{1}"])

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


def test_branches_always_visible(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)
        git("branch", ["test"])
        git("checkout", ["master"])

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

        git("branch", ["-D", "test"])

        with io.StringIO() as out:
            assert smartlog(out=out) == 0
            compare(
                actual=out.getvalue(),
                expected="""\
@ f777ecc9 (master) create initial.txt
""",
            )


def test_unhide(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)
        git("checkout", ["master"])

        with io.StringIO() as out:
            assert unhide(out=out, hashes=["96d1c37"]) == 0
            compare(
                actual=out.getvalue(),
                expected="""\
Unhid commit: 96d1c37a
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
Unhid commit: 96d1c37a
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
