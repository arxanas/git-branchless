import io

import py
from branchless.hide import hide
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
* fe65c1fe create test2.txt
|
| o 62fc20d2 create test1.txt
|/
o f777ecc9 create initial.txt
""",
            )

        with io.StringIO() as out:
            assert hide(out=out, hash="62fc20d2") == 0
            compare(
                actual=out.getvalue(),
                expected="""\
Hid commit: 62fc20d2
To unhide this commit, run: git checkout 62fc20d2
""",
            )

        with io.StringIO() as out:
            assert smartlog(out=out) == 0
            compare(
                actual=out.getvalue(),
                expected="""\
* fe65c1fe create test2.txt
|
o f777ecc9 create initial.txt
""",
            )


def test_hide_bad_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd(), io.StringIO() as out:
        git_init_repo()
        assert hide(out=out, hash="abc123") == 1
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
o 70deb1e2 create test3.txt
|
o 96d1c37a create test2.txt
|
o 62fc20d2 create test1.txt
|
* f777ecc9 create initial.txt
""",
            )

        with io.StringIO() as out:
            assert hide(out=out, hash="96d1c37a") == 0

        with io.StringIO() as out:
            smartlog(out=out)
            compare(
                actual=out.getvalue(),
                expected="""\
o 62fc20d2 create test1.txt
|
* f777ecc9 create initial.txt
""",
            )


def test_hide_already_hidden_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)

        with io.StringIO() as out:
            assert hide(out=out, hash="62fc20d2") == 0

        with io.StringIO() as out:
            assert hide(out=out, hash="62fc20d2") == 0
            compare(
                actual=out.getvalue(),
                expected="""\
Hid commit: 62fc20d2
(It was already hidden, so this operation had no effect.)
To unhide this commit, run: git checkout 62fc20d2
""",
            )


def test_hide_current_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test", time=1)

        with io.StringIO() as out:
            assert hide(out=out, hash="HEAD") == 0

        with io.StringIO() as out:
            assert smartlog(out=out) == 0
            compare(
                expected=out.getvalue(),
                actual="""\
% 3df4b935 create test.txt
|
o f777ecc9 create initial.txt
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
            assert hide(out=out, hash="HEAD^") == 0
            assert hide(out=out, hash="70deb1e2") == 0

        with io.StringIO() as out:
            assert smartlog(out=out) == 0
            compare(
                actual=out.getvalue(),
                expected="""\
* 96d1c37a create test2.txt
|
x 62fc20d2 create test1.txt
|
o f777ecc9 create initial.txt
""",
            )
