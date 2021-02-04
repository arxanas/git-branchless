import io

from branchless.__main__ import main
from helpers import Git


def test_help(git: Git) -> None:
    with io.StringIO() as out, io.StringIO() as err:
        assert (
            main(["--help"], out=out, err=err, git_executable=git.git_executable) == 0
        )
        assert "usage: branchless" in out.getvalue()

    with io.StringIO() as out, io.StringIO() as err:
        assert main([], out=out, err=err, git_executable=git.git_executable) == 1
        assert "usage: branchless" in out.getvalue()
