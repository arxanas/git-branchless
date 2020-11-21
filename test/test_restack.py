import sys

import py
from _pytest.capture import CaptureFixture

from branchless.restack import restack
from branchless.smartlog import smartlog
from helpers import (
    clear_capture,
    compare,
    git,
    git_commit_file,
    git_detach_head,
    git_init_repo,
)


def test_restack_amended_commit(
    tmpdir: py.path.local, capfd: CaptureFixture[str]
) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)
        git_commit_file(name="test3", time=3)
        git("checkout", ["HEAD^^"])
        git("commit", ["--amend", "-m", "amend test1.txt"])

        assert smartlog(out=sys.stdout) == 0
        (out, _err) = capfd.readouterr()
        compare(
            actual=out,
            expected="""\
O f777ecc9 (master) create initial.txt
|\\
| @ 024c35ce amend test1.txt
|
x 62fc20d2 create test1.txt
|
o 96d1c37a create test2.txt
|
o 70deb1e2 create test3.txt
""",
        )
        clear_capture(capfd)

        restack(out=sys.stdout, preserve_timestamps=True)
        (out, _err) = capfd.readouterr()
        compare(
            actual=out,
            expected="""\
branchless: git rebase 62fc20d2a290daea0d52bdc2ed2ad4be6491010e \
96d1c37a3d4363611c49f7e52186e189a04c531f \
--onto 024c35ce32dae6b12e981963465ee8a62b7eff9b --committer-date-is-author-date
First, rewinding head to replay your work on top of it...
Applying: create test2.txt
branchless: git rebase 96d1c37a3d4363611c49f7e52186e189a04c531f \
70deb1e28791d8e7dd5a1f0c871a51b91282562f \
--onto 93ec27e52914527f98092b572e28e98ca4fbc25b --committer-date-is-author-date
First, rewinding head to replay your work on top of it...
Applying: create test3.txt
branchless: no more abandoned commits to restack
branchless: no more abandoned branches to restack
branchless: git checkout 024c35ce32dae6b12e981963465ee8a62b7eff9b
O f777ecc9 (master) create initial.txt
|
@ 024c35ce amend test1.txt
|
o 93ec27e5 create test2.txt
|
o 60a05cbd create test3.txt
""",
        )
        clear_capture(capfd)


def test_restack_consecutive_rewrites(
    tmpdir: py.path.local, capfd: CaptureFixture[str]
) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git_commit_file(name="test2", time=2)
        git_commit_file(name="test3", time=3)
        git("checkout", ["HEAD^^"])
        git("commit", ["--amend", "-m", "amend test1.txt v1"])
        git("commit", ["--amend", "-m", "amend test1.txt v2"])

        restack(out=sys.stdout, preserve_timestamps=True)
        (out, _err) = capfd.readouterr()
        compare(
            actual=out,
            expected="""\
branchless: git rebase 62fc20d2a290daea0d52bdc2ed2ad4be6491010e \
96d1c37a3d4363611c49f7e52186e189a04c531f \
--onto 662b451fb905b92404787e024af717ced49e3045 --committer-date-is-author-date
First, rewinding head to replay your work on top of it...
Applying: create test2.txt
branchless: git rebase 96d1c37a3d4363611c49f7e52186e189a04c531f \
70deb1e28791d8e7dd5a1f0c871a51b91282562f \
--onto 8980f82a39a486a8e75be2c5d401f5cb46f59a6a --committer-date-is-author-date
First, rewinding head to replay your work on top of it...
Applying: create test3.txt
branchless: no more abandoned commits to restack
branchless: no more abandoned branches to restack
branchless: git checkout 662b451fb905b92404787e024af717ced49e3045
O f777ecc9 (master) create initial.txt
|
@ 662b451f amend test1.txt v2
|
o 8980f82a create test2.txt
|
o b71a20e4 create test3.txt
""",
        )
        clear_capture(capfd)


def test_move_abandoned_branch(
    tmpdir: py.path.local, capfd: CaptureFixture[str]
) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_commit_file(name="test1", time=1)
        git_detach_head()
        git("commit", ["--amend", "-m", "amend test1.txt v1"])
        git("commit", ["--amend", "-m", "amend test1.txt v2"])

        restack(out=sys.stdout, preserve_timestamps=True)
        (out, _err) = capfd.readouterr()
        compare(
            actual=out,
            expected="""\
branchless: no more abandoned commits to restack
branchless: git branch -f master 662b451fb905b92404787e024af717ced49e3045
branchless: no more abandoned branches to restack
branchless: git checkout 662b451fb905b92404787e024af717ced49e3045
:
@ 662b451f (master) amend test1.txt v2
""",
        )
        clear_capture(capfd)
