from branchless.restack import restack
from branchless.smartlog import smartlog
from helpers import Git, capture, compare


def preprocess_output(output: str) -> str:
    return "".join(
        line
        for line in output.splitlines(keepends=True)
        if "First, rewinding head" not in line
        if "Applying:" not in line
    )


def test_restack_amended_commit(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.commit_file(name="test3", time=3)
    git.run("checkout", ["HEAD^^"])
    git.run("commit", ["--amend", "-m", "amend test1.txt"])

    with capture("out") as out:
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

    with capture("out") as out, capture("err") as err:
        restack(
            out=out.stream,
            err=err.stream,
            git_executable=git.git_executable,
            preserve_timestamps=True,
        )
        compare(
            actual=preprocess_output(out.getvalue()),
            expected=f"""\
branchless: {git.git_executable} rebase 62fc20d2a290daea0d52bdc2ed2ad4be6491010e \
96d1c37a3d4363611c49f7e52186e189a04c531f \
--onto 024c35ce32dae6b12e981963465ee8a62b7eff9b --committer-date-is-author-date
branchless: {git.git_executable} rebase 96d1c37a3d4363611c49f7e52186e189a04c531f \
70deb1e28791d8e7dd5a1f0c871a51b91282562f \
--onto 8cd7de680cafaba911d09f430d2bafb1169d6e65 --committer-date-is-author-date
branchless: no more abandoned commits to restack
branchless: no more abandoned branches to restack
branchless: {git.git_executable} checkout 024c35ce32dae6b12e981963465ee8a62b7eff9b
O f777ecc9 (master) create initial.txt
|
@ 024c35ce amend test1.txt
|
o 8cd7de68 create test2.txt
|
o b9a0491a create test3.txt
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

    with capture("out") as out, capture("err") as err:
        restack(
            out=out.stream,
            err=err.stream,
            git_executable=git.git_executable,
            preserve_timestamps=True,
        )
        compare(
            actual=preprocess_output(out.getvalue()),
            expected=f"""\
branchless: {git.git_executable} rebase 62fc20d2a290daea0d52bdc2ed2ad4be6491010e \
96d1c37a3d4363611c49f7e52186e189a04c531f \
--onto 662b451fb905b92404787e024af717ced49e3045 --committer-date-is-author-date
branchless: {git.git_executable} rebase 96d1c37a3d4363611c49f7e52186e189a04c531f \
70deb1e28791d8e7dd5a1f0c871a51b91282562f \
--onto 8e9bbde339899eaabf48cf0d8b89d52144db94e1 --committer-date-is-author-date
branchless: no more abandoned commits to restack
branchless: no more abandoned branches to restack
branchless: {git.git_executable} checkout 662b451fb905b92404787e024af717ced49e3045
O f777ecc9 (master) create initial.txt
|
@ 662b451f amend test1.txt v2
|
o 8e9bbde3 create test2.txt
|
o 9dc6dd07 create test3.txt
""",
        )


def test_move_abandoned_branch(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.detach_head()
    git.run("commit", ["--amend", "-m", "amend test1.txt v1"])
    git.run("commit", ["--amend", "-m", "amend test1.txt v2"])

    with capture("out") as out, capture("err") as err:
        restack(
            out=out.stream,
            err=err.stream,
            git_executable=git.git_executable,
            preserve_timestamps=True,
        )
        compare(
            actual=out.getvalue(),
            expected=f"""\
branchless: no more abandoned commits to restack
branchless: {git.git_executable} branch -f master 662b451fb905b92404787e024af717ced49e3045
branchless: no more abandoned branches to restack
branchless: {git.git_executable} checkout 662b451fb905b92404787e024af717ced49e3045
:
@ 662b451f (master) amend test1.txt v2
""",
        )


def test_amended_initial_commit(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.run("checkout", ["HEAD^"])
    git.run("commit", ["--amend", "-m", "new initial commit"])

    with capture("out") as out:
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

    with capture("out") as out, capture("err") as err:
        assert (
            restack(
                out=out.stream,
                err=err.stream,
                git_executable=git.git_executable,
                preserve_timestamps=True,
            )
            == 0
        )
        compare(
            actual=preprocess_output(out.getvalue()),
            expected=f"""\
branchless: {git.git_executable} rebase f777ecc9b0db5ed372b2615695191a8a17f79f24 \
62fc20d2a290daea0d52bdc2ed2ad4be6491010e \
--onto 9a9f929a0d4f052ff5d58bedd97b2f761120f8ed \
--committer-date-is-author-date
branchless: no more abandoned commits to restack
branchless: {git.git_executable} branch -f master 6d85943be6d6e5941d5479f1059d02ebf1c8e307
branchless: no more abandoned branches to restack
branchless: {git.git_executable} checkout 9a9f929a0d4f052ff5d58bedd97b2f761120f8ed
@ 9a9f929a new initial commit
|
O 6d85943b (master) create test1.txt
""",
        )


def test_restack_amended_master(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=1)
    git.detach_head()
    git.run("checkout", ["HEAD^"])
    git.run("commit", ["--amend", "-m", "amended test1"])

    with capture("out") as out, capture("err") as err:
        assert (
            restack(
                out=out.stream,
                err=err.stream,
                git_executable=git.git_executable,
                preserve_timestamps=True,
            )
            == 0
        )
        compare(
            actual=preprocess_output(out.getvalue()),
            expected=f"""\
branchless: {git.git_executable} rebase 62fc20d2a290daea0d52bdc2ed2ad4be6491010e \
142901d553f71d2711a3754424a67191397915c4 \
--onto ae94dc2a748bc0965c88fcf3edac2e30074ff7e2 \
--committer-date-is-author-date
branchless: no more abandoned commits to restack
branchless: {git.git_executable} branch -f master 38528a1a0effd235d909f2fa9ec9a7dd7aad77b5
branchless: no more abandoned branches to restack
branchless: {git.git_executable} checkout ae94dc2a748bc0965c88fcf3edac2e30074ff7e2
:
@ ae94dc2a amended test1
|
O 38528a1a (master) create test2.txt
""",
        )
