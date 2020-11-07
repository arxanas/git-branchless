import io

from branchless.__main__ import main


def test_help() -> None:
    with io.StringIO() as out:
        assert main(["--help"], out=out) == 0
        assert "usage: branchless" in out.getvalue()
