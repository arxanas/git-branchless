from _pytest.capture import CaptureFixture

from helpers import Git


def clear_capture(capture: CaptureFixture[str]) -> None:
    capture.readouterr()


def test_abandoned_commit_message(git: Git, capfd: CaptureFixture[str]) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)

    clear_capture(capfd)
    git.run("commit", ["--amend", "-m", "amend test1"])
    (_out, err) = capfd.readouterr()
    assert (
        err
        == """\
branchless: processing commit
branchless: processing 1 rewritten commit
"""
    )

    git.commit_file(name="test2", time=2)
    git.run("checkout", ["HEAD^"])
    git.run("branch", ["-f", "master"])

    clear_capture(capfd)
    git.run("commit", ["--amend", "-m", "amend test1 again"])
    (_out, err) = capfd.readouterr()
    assert (
        err
        == """\
branchless: processing commit
branchless: processing 1 rewritten commit
branchless: This operation abandoned 1 commit and 1 branch (master)!
branchless: Consider running one of the following:
branchless:   - git restack: re-apply the abandoned commits/branches
branchless:     (this is most likely what you want to do)
branchless:   - git smartlog: assess the situation
branchless:   - git hide [<commit>...]: hide the commits from the smartlog
branchless:   - git undo: undo the operation
branchless:   - git config branchless.restack.warnAbandoned false: suppress this message
"""
    )


def test_abandoned_branch_message(git: Git, capfd: CaptureFixture[str]) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.run("branch", ["abc"])
    git.detach_head()

    clear_capture(capfd)
    git.run("commit", ["--amend", "-m", "amend test1"])
    (_out, err) = capfd.readouterr()
    assert (
        err
        == """\
branchless: processing commit
branchless: processing 1 rewritten commit
branchless: This operation abandoned 2 branches (abc, master)!
branchless: Consider running one of the following:
branchless:   - git restack: re-apply the abandoned commits/branches
branchless:     (this is most likely what you want to do)
branchless:   - git smartlog: assess the situation
branchless:   - git hide [<commit>...]: hide the commits from the smartlog
branchless:   - git undo: undo the operation
branchless:   - git config branchless.restack.warnAbandoned false: suppress this message
"""
    )
