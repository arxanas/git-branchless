from branchless.navigation import next, prev
from helpers import Git, capture, compare


def test_prev(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)

    with capture() as out, capture() as err:
        assert prev(out=out.stream, err=err.stream, num_commits=None) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
branchless: git checkout HEAD^
@ f777ecc9 create initial.txt
|
O 62fc20d2 (master) create test1.txt
""",
        )

    with capture() as out, capture() as err:
        assert prev(out=out.stream, err=err.stream, num_commits=None) == 1
        compare(
            actual=out.getvalue() + err.getvalue(),
            expected="""\
branchless: git checkout HEAD^
error: pathspec 'HEAD^' did not match any file(s) known to git
""",
        )


def test_prev_multiple(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)

    with capture() as out, capture() as err:
        assert prev(out=out.stream, err=err.stream, num_commits=2) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
branchless: git checkout HEAD~2
@ f777ecc9 create initial.txt
:
O 96d1c37a (master) create test2.txt
""",
        )


def test_next_multiple(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.run("checkout", ["master"])

    with capture() as out, capture() as err:
        assert next(out=out.stream, err=err.stream, num_commits=2, towards=None) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
branchless: git checkout 96d1c37a3d4363611c49f7e52186e189a04c531f
O f777ecc9 (master) create initial.txt
|
o 62fc20d2 create test1.txt
|
@ 96d1c37a create test2.txt
""",
        )


def test_next_ambiguous(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.run("checkout", ["master"])
    git.detach_head()
    git.commit_file(name="test2", time=2)
    git.run("checkout", ["master"])
    git.detach_head()
    git.commit_file(name="test3", time=3)
    git.run("checkout", ["master"])

    with capture() as out, capture() as err:
        assert next(out=out.stream, err=err.stream, num_commits=None, towards=None) == 1
        compare(
            actual=out.getvalue(),
            expected="""\
Found multiple possible next commits to go to after traversing 0 children:
  - 62fc20d2 create test1.txt (oldest)
  - fe65c1fe create test2.txt
  - 98b9119d create test3.txt (newest)
(Pass --oldest (-o) or --newest (-n) to select between ambiguous next commits)
""",
        )

    with capture() as out, capture() as err:
        assert (
            next(out=out.stream, err=err.stream, num_commits=None, towards="oldest")
            == 0
        )
        compare(
            actual=out.getvalue(),
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

    git.run("checkout", ["master"])

    with capture() as out, capture() as err:
        assert (
            next(out=out.stream, err=err.stream, num_commits=None, towards="newest")
            == 0
        )
        compare(
            actual=out.getvalue(),
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


def test_next_on_master(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.detach_head()
    git.commit_file(name="test3", time=3)
    git.run("checkout", ["HEAD^^"])

    with capture() as out, capture() as err:
        assert next(out=out.stream, err=err.stream, num_commits=2, towards=None) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
branchless: git checkout 70deb1e28791d8e7dd5a1f0c871a51b91282562f
:
O 96d1c37a (master) create test2.txt
|
@ 70deb1e2 create test3.txt
""",
        )


def test_next_on_master2(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.detach_head()
    git.commit_file(name="test2", time=2)
    git.commit_file(name="test3", time=3)
    git.run("checkout", ["HEAD^"])

    with capture() as out, capture() as err:
        assert next(out=out.stream, err=err.stream, num_commits=None, towards=None) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
branchless: git checkout 70deb1e28791d8e7dd5a1f0c871a51b91282562f
:
O 62fc20d2 (master) create test1.txt
|
o 96d1c37a create test2.txt
|
@ 70deb1e2 create test3.txt
""",
        )
