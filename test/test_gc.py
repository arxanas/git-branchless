import pytest

from helpers import Git, compare


def test_gc(git: Git) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.run("checkout", ["HEAD^"])

    git.run("gc", ["--prune=now"])
    compare(
        actual=git.run("smartlog"),
        expected="""\
@ f777ecc9 (master) create initial.txt
|
o 62fc20d2 create test1.txt
""",
    )

    git.run("hide", ["62fc20d2"])
    compare(
        actual=git.run("branchless", ["gc"]),
        expected="""\
branchless: collecting garbage
""",
    )

    git.run("gc", ["--prune=now"])
    compare(
        actual=git.run("smartlog"),
        expected="""\
@ f777ecc9 (master) create initial.txt
""",
    )

    repo = git.get_repo()
    with pytest.raises(KeyError):
        repo["62fc20d2"]
