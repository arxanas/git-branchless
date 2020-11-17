import sys

import py
from _pytest.capture import CaptureFixture

from branchless.navigation import next, prev
from helpers import (
    clear_capture,
    compare,
    git,
    git_commit_file,
    git_detach_head,
    git_init_repo,
)


def test_prev(tmpdir: py.path.local, capfd: CaptureFixture[str]) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_commit_file(name="test1", time=1)

        assert prev(out=sys.stdout, num_commits=None) == 0
        (out, _err) = capfd.readouterr()
        compare(
            actual=out,
            expected="""\
branchless: git checkout HEAD^
@ f777ecc9 create initial.txt
|
O 62fc20d2 (master) create test1.txt
""",
        )
        clear_capture(capfd)

        assert prev(out=sys.stdout, num_commits=None) == 1
        (out, err) = capfd.readouterr()
        compare(
            actual=out + err,
            expected="""\
branchless: git checkout HEAD^
error: pathspec 'HEAD^' did not match any file(s) known to git
""",
        )
        clear_capture(capfd)


def test_prev_multiple(tmpdir: py.path.local, capfd: CaptureFixture[str]) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)

        assert prev(out=sys.stdout, num_commits=2) == 0
        (out, _err) = capfd.readouterr()
        compare(
            actual=out,
            expected="""\
branchless: git checkout HEAD~2
@ f777ecc9 create initial.txt
:
O 96d1c37a (master) create test2.txt
""",
        )
        clear_capture(capfd)


def test_next_multiple(tmpdir: py.path.local, capfd: CaptureFixture[str]) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)
        git("checkout", ["master"])

        assert next(out=sys.stdout, num_commits=2, towards=None) == 0
        (out, _err) = capfd.readouterr()
        compare(
            actual=out,
            expected="""\
branchless: git checkout 96d1c37a3d4363611c49f7e52186e189a04c531f
O f777ecc9 (master) create initial.txt
|
o 62fc20d2 create test1.txt
|
@ 96d1c37a create test2.txt
""",
        )
        clear_capture(capfd)


def test_next_ambiguous(tmpdir: py.path.local, capfd: CaptureFixture[str]) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git("checkout", ["master"])
        git_detach_head()
        git_commit_file(name="test2", time=2)
        git("checkout", ["master"])
        git_detach_head()
        git_commit_file(name="test3", time=3)
        git("checkout", ["master"])

        assert next(out=sys.stdout, num_commits=None, towards=None) == 1
        (out, _err) = capfd.readouterr()
        compare(
            actual=out,
            expected="""\
Found multiple possible next commits to go to after traversing 0 children:
  - 62fc20d2 create test1.txt (oldest)
  - fe65c1fe create test2.txt
  - 98b9119d create test3.txt (newest)
(Pass --oldest (-o) or --newest (-n) to select between ambiguous next commits)
""",
        )
        clear_capture(capfd)

        assert next(out=sys.stdout, num_commits=None, towards="oldest") == 0
        (out, _err) = capfd.readouterr()
        compare(
            actual=out,
            expected="""\
branchless: git checkout 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
O f777ecc9 (master) create initial.txt
|\\
| @ 62fc20d2 create test1.txt
|\\
| o fe65c1fe create test2.txt
|
o 98b9119d create test3.txt
""",
        )
        clear_capture(capfd)

        git("checkout", ["master"])

        assert next(out=sys.stdout, num_commits=None, towards="newest") == 0
        (out, _err) = capfd.readouterr()
        compare(
            actual=out,
            expected="""\
branchless: git checkout 98b9119d16974f372e76cb64a3b77c528fc0b18b
O f777ecc9 (master) create initial.txt
|\\
| o 62fc20d2 create test1.txt
|\\
| o fe65c1fe create test2.txt
|
@ 98b9119d create test3.txt
""",
        )
        clear_capture(capfd)
