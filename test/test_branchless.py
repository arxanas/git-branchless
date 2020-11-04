import io

from branchless.__main__ import main


def test_help() -> None:
    with io.StringIO() as out:
        main(["--help"], out=out)
        assert "usage: branchless" in out.getvalue()
