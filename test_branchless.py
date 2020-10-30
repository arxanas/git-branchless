import py
import subprocess
import pytest
from branchless import main

from typing import Any, Optional, List


GIT_PATH = "/opt/twitter_mde/bin/git"

DUMMY_NAME = "Testy McTestface"
DUMMY_EMAIL = "test@example.com"
DUMMY_DATE = "Wed 29 Oct 12:34:56 2020 PDT"


def git(command: str, args: Optional[List[str]] = None) -> str:
    if args is None:
        args = []
    args = [GIT_PATH, command, *args]

    # Required for determinism, as these values will be baked into the commit
    # hash.
    env = {
        "GIT_AUTHOR_NAME": DUMMY_NAME,
        "GIT_AUTHOR_EMAIL": DUMMY_EMAIL,
        "GIT_AUTHOR_DATE": DUMMY_DATE,
        "GIT_COMMITTER_NAME": DUMMY_NAME,
        "GIT_COMMITTER_EMAIL": DUMMY_EMAIL,
        "GIT_COMMITTER_DATE": DUMMY_DATE,
    }

    result = subprocess.run(args, stdout=subprocess.PIPE, env=env)
    return result.stdout.decode()


def init_git(cwd: py.path.local) -> None:
    git("init")
    cwd.join("initial.txt").write("initial")
    git("add", ["."])
    git("commit", ["-m", "Initial commit"])


def test_help(capsys: Any) -> None:
    with pytest.raises(SystemExit):
        main(["--help"])
    assert "usage: branchless" in capsys.readouterr().out


def test_init(tmpdir: py.path.local, capsys: Any) -> None:
    with tmpdir.as_cwd():
        init_git(tmpdir)

        tmpdir.join("test2.txt").write("hello")
        git("add", ["."])
        git("commit", ["-m", "create test2.txt"])

        main(["smartlog"])
        assert (
            capsys.readouterr().out
            == """\
d4dd7470 create test2.txt
"""
        )
