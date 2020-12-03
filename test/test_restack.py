import pytest

from branchless.restack import restack
from branchless.smartlog import smartlog
from helpers import Git, capture, compare


def test_restack_amended_commit(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.commit_file(name="test3", time=3)
    git.run("checkout", ["HEAD^^"])
    git.run("commit", ["--amend", "-m", "amend test1.txt"])

    with capture() as out:
        assert smartlog(out=out.stream) == 0
        compare(
            actual=out.getvalue(),
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

    with capture() as out, capture() as err:
        restack(out=out.stream, err=err.stream, preserve_timestamps=True)
        compare(
            actual=out.getvalue(),
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


def test_restack_consecutive_rewrites(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.commit_file(name="test3", time=3)
    git.run("checkout", ["HEAD^^"])
    git.run("commit", ["--amend", "-m", "amend test1.txt v1"])
    git.run("commit", ["--amend", "-m", "amend test1.txt v2"])

    with capture() as out, capture() as err:
        restack(out=out.stream, err=err.stream, preserve_timestamps=True)
        compare(
            actual=out.getvalue(),
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


def test_move_abandoned_branch(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.detach_head()
    git.run("commit", ["--amend", "-m", "amend test1.txt v1"])
    git.run("commit", ["--amend", "-m", "amend test1.txt v2"])

    with capture() as out, capture() as err:
        restack(out=out.stream, err=err.stream, preserve_timestamps=True)
        compare(
            actual=out.getvalue(),
            expected="""\
branchless: no more abandoned commits to restack
branchless: git branch -f master 662b451fb905b92404787e024af717ced49e3045
branchless: no more abandoned branches to restack
branchless: git checkout 662b451fb905b92404787e024af717ced49e3045
:
@ 662b451f (master) amend test1.txt v2
""",
        )


@pytest.mark.xfail
def test_amended_initial_commit(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.run("checkout", ["HEAD^"])
    git.run("commit", ["--amend", "-m", "new initial commit"])

    with capture() as out:
        # Pathological output.
        assert smartlog(out=out.stream) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
@ 9a9f929a new initial commit

X f777ecc9 create initial.txt
|
O 62fc20d2 (master) create test1.txt
""",
        )

    with capture() as out, capture() as err:
        assert restack(out=out.stream, err=err.stream, preserve_timestamps=True) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
branchless: git rebase f777ecc9b0db5ed372b2615695191a8a17f79f24 \
62fc20d2a290daea0d52bdc2ed2ad4be6491010e \
--onto 9a9f929a0d4f052ff5d58bedd97b2f761120f8ed \
--committer-date-is-author-date
First, rewinding head to replay your work on top of it...
Applying: create test1.txt
branchless: no more abandoned commits to restack
branchless: git branch -f master e18bf94239100139cb8d7a279c188981a8e7a445
branchless: no more abandoned branches to restack
branchless: git checkout 9a9f929a0d4f052ff5d58bedd97b2f761120f8ed
@ 9a9f929a new initial commit
|
O e18bf942 (master) create test1.txt
""",
        )


@pytest.mark.xfail
def test_restack_amended_master(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=1)
    git.detach_head()
    git.run("checkout", ["HEAD^"])
    git.run("commit", ["--amend", "-m", "amended test1"])

    with capture() as out, capture() as err:
        assert restack(out=out.stream, err=err.stream, preserve_timestamps=True) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
branchless: git rebase 62fc20d2a290daea0d52bdc2ed2ad4be6491010e \
142901d553f71d2711a3754424a67191397915c4 \
--onto ae94dc2a748bc0965c88fcf3edac2e30074ff7e2 \
--committer-date-is-author-date
First, rewinding head to replay your work on top of it...
Applying: create test2.txt
branchless: no more abandoned commits to restack
branchless: git branch -f master 1b1619f6ea3e930df4932981f2680eeb33bf17b0
branchless: no more abandoned branches to restack
branchless: git checkout ae94dc2a748bc0965c88fcf3edac2e30074ff7e2
:
@ ae94dc2a amended test1
|
O 1b1619f6 (master) create test2.txt
""",
        )
