import io
import time
from unittest.mock import patch

import pytest

from branchless.metadata import RelativeTimeProvider
from branchless.smartlog import smartlog
from helpers import Git, compare


def test_multiple_branches_for_same_commit(git: Git) -> None:
    git.init_repo()
    git.run("branch", ["abc"])
    git.run("branch", ["xyz"])

    # Ensure that the branches are always in alphabetical order.
    for i in range(10):
        with io.StringIO() as out:
            assert smartlog(out=out) == 0
            compare(
                actual=out.getvalue(),
                expected="""\
@ f777ecc9 (abc, master, xyz) create initial.txt
""",
            )


def test_differential_revision_provider(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="name1", time=1)
    git.run(
        "commit",
        [
            "--amend",
            "-m",
            """\
create test1.txt

Differential Revision: https://some-phabricator-url.example/D12345
""",
        ],
    )

    with io.StringIO() as out:
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
:
@ 4d4ded9a (master) D12345 create test1.txt
""",
        )


def test_relative_time_provider(git: Git) -> None:
    git.init_repo()
    git.run("config", ["branchless.commitMetadata.relativeTime", "true"])

    initial_commit_timestamp = int(git.run("show", ["-s", "--format=%ct"]).strip())
    with io.StringIO() as out, patch.object(
        time, "time", return_value=initial_commit_timestamp + 10
    ):
        assert smartlog(out=out) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
@ f777ecc9 10s (master) create initial.txt
""",
        )


@pytest.mark.parametrize(
    ("delta", "expected"),
    [
        # Could improve formatting for times in the past.
        (-100000, "-100000s"),
        (-1, "-1s"),
        (0, "0s"),
        (10, "10s"),
        (60, "1m"),
        (90, "1m"),
        (120, "2m"),
        (135, "2m"),
        (60 * 45, "45m"),
        (60 * 60 - 1, "59m"),
        (60 * 60, "1h"),
        (60 * 60 * 24 * 3, "3d"),
        (60 * 60 * 24 * 300, "300d"),
        (60 * 60 * 24 * 400, "1y"),
    ],
)
def test_relative_time_descriptions(delta: int, expected: str) -> None:
    actual = RelativeTimeProvider.describe_time_delta(now=delta, previous_time=0)
    assert expected == actual
