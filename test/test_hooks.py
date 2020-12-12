from _pytest.capture import CaptureFixture

from helpers import Git


def clear_capture(capture: CaptureFixture[str]) -> None:
    capture.readouterr()


def preprocess_stderr(stderr: str) -> str:
    return "".join(
        line
        for line in stderr.replace("\r", "\n").splitlines(keepends=True)
        if not line.isspace()
        if not line.startswith("branchless: processing")
    )


def test_abandoned_commit_message(git: Git, capfd: CaptureFixture[str]) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)

    clear_capture(capfd)
    git.run("commit", ["--amend", "-m", "amend test1"])
    (_out, err) = capfd.readouterr()
    err = preprocess_stderr(err)
    assert err == ""

    git.commit_file(name="test2", time=2)
    git.run("checkout", ["HEAD^"])
    git.run("branch", ["-f", "master"])

    clear_capture(capfd)
    git.run("commit", ["--amend", "-m", "amend test1 again"])
    (_out, err) = capfd.readouterr()
    err = preprocess_stderr(err)
    assert (
        err
        == """\
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
    err = preprocess_stderr(err)
    assert (
        err
        == """\
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


def test_fixup_no_abandoned_commit_message(
    git: Git, capfd: CaptureFixture[str]
) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.run("commit", ["--amend", "-m", "fixup! create test1.txt"])
    git.commit_file(name="test3", time=3)
    git.run("commit", ["--amend", "-m", "fixup! create test1.txt"])

    clear_capture(capfd)
    git.run("rebase", ["-i", "master", "--autosquash"])
    (_out, err) = capfd.readouterr()
    err = preprocess_stderr(err)
    assert (
        err
        == """\
Rebasing (2/3)
Rebasing (3/3)
Successfully rebased and updated detached HEAD.
"""
    )


def test_rebase_individual_commit(git: Git, capfd: CaptureFixture[str]) -> None:
    git.requires_git29()

    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.run("checkout", ["HEAD^"])
    git.commit_file(name="test2", time=2)
    git.commit_file(name="test3", time=3)

    clear_capture(capfd)
    git.run("rebase", ["master", "HEAD^", "--onto", "master"])
    (_out, err) = capfd.readouterr()
    err = preprocess_stderr(err)
    assert (
        err
        == """\
Rebasing (1/1)
branchless: This operation abandoned 1 commit!
branchless: Consider running one of the following:
branchless:   - git restack: re-apply the abandoned commits/branches
branchless:     (this is most likely what you want to do)
branchless:   - git smartlog: assess the situation
branchless:   - git hide [<commit>...]: hide the commits from the smartlog
branchless:   - git undo: undo the operation
branchless:   - git config branchless.restack.warnAbandoned false: suppress this message
Successfully rebased and updated detached HEAD.
"""
    )


def test_interactive_rebase_noop(git: Git, capfd: CaptureFixture[str]) -> None:
    git.init_repo()
    git.detach_head()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)

    clear_capture(capfd)
    git.run("rebase", ["-i", "master"])
    (_out, err) = capfd.readouterr()
    err = preprocess_stderr(err)
    assert (
        err
        == """\
Successfully rebased and updated detached HEAD.
"""
    )
