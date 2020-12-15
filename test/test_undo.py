import io
from contextlib import contextmanager
from typing import Iterator, List, Sequence
from unittest.mock import patch

import pytest
import readchar

import branchless.undo
from branchless.eventlog import Event, RefUpdateEvent
from branchless.smartlog import smartlog
from branchless.undo import _optimize_inverse_events, undo
from helpers import Git, capture, compare


@contextmanager
def mock_keypresses(keypresses: Sequence[str]) -> Iterator[None]:
    with patch.object(readchar, "readkey", side_effect=list(keypresses) + ["q"]):
        yield


@contextmanager
def mock_inputs(inputs: Sequence[str]) -> Iterator[None]:
    with patch.object(branchless.undo, "input", side_effect=inputs):
        yield


@contextmanager
def mock_time_difference() -> Iterator[None]:
    relative_time_provider = branchless.undo.RelativeTimeProvider  # type: ignore[attr-defined]
    with patch.object(relative_time_provider, "describe_time_delta", return_value="?s"):
        yield


def test_undo_help(git: Git) -> None:
    git.init_repo()

    with io.StringIO() as out, io.StringIO() as err, mock_keypresses(["h"]):
        assert undo(out=out, err=err, git_executable=git.git_executable) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 (master) create initial.txt
There are no previous available events.
HOW TO USE
Use `git undo` to view and revert to previous states of the repository.

h/?: Show this help.
q: Quit.
p/n or <left>/<right>: View next/previous state.
g: Go to a provided event ID.
<enter>: Revert the repository to the given state (requires confirmation).

You can also copy a commit hash from the past and manually run `git unhide`
or `git rebase` on it.
""",
        )


def test_undo_navigate(git: Git) -> None:
    git.requires_git29()

    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)

    with io.StringIO() as out, io.StringIO() as err, mock_time_difference(), mock_keypresses(
        ["p", "n"]
    ):
        assert undo(out=out, err=err, git_executable=git.git_executable) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
:
@ 96d1c37a (master) create test2.txt
Repo after event 6 (?s ago). Press 'h' for help, 'q' to quit.
Commit 96d1c37a create test2.txt

:
@ 96d1c37a (master) create test2.txt
Repo after event 5 (?s ago). Press 'h' for help, 'q' to quit.
Move branch master from 62fc20d2 create test1.txt
                     to 96d1c37a create test2.txt
:
@ 96d1c37a (master) create test2.txt
Repo after event 6 (?s ago). Press 'h' for help, 'q' to quit.
Commit 96d1c37a create test2.txt

""",
        )


def test_undo_go_to_event(git: Git) -> None:
    git.requires_git29()

    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)

    with io.StringIO() as out, io.StringIO() as err, mock_time_difference(), mock_keypresses(
        ["G"]
    ), mock_inputs(
        ["1"]
    ):
        assert undo(out=out, err=err, git_executable=git.git_executable) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
:
@ 96d1c37a (master) create test2.txt
Repo after event 6 (?s ago). Press 'h' for help, 'q' to quit.
Commit 96d1c37a create test2.txt

:
@ 62fc20d2 create test1.txt
|
O 96d1c37a (master) create test2.txt
Repo after event 1 (?s ago). Press 'h' for help, 'q' to quit.
Check out from f777ecc9 create initial.txt
            to 62fc20d2 create test1.txt
""",
        )

    with io.StringIO() as out, io.StringIO() as err, mock_time_difference(), mock_keypresses(
        ["G"]
    ), mock_inputs(
        ["foo"]
    ):
        assert undo(out=out, err=err, git_executable=git.git_executable) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
:
@ 96d1c37a (master) create test2.txt
Repo after event 6 (?s ago). Press 'h' for help, 'q' to quit.
Commit 96d1c37a create test2.txt

Invalid event ID: foo
""",
        )


def test_undo_hide(git: Git) -> None:
    git.requires_git29()

    git.init_repo()
    git.run("checkout", ["-b", "test1"])
    git.commit_file(name="test1", time=1)
    git.run("checkout", ["HEAD^"])
    git.commit_file(name="test2", time=2)
    git.run("hide", ["test1"])
    git.run("branch", ["-D", "test1"])

    with io.StringIO() as out, io.StringIO() as err, mock_time_difference(), mock_keypresses(
        ["p", "p", readchar.key.ENTER]
    ), mock_inputs(
        ["y"]
    ):
        assert undo(out=out, err=err, git_executable=git.git_executable) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 (master) create initial.txt
|
@ fe65c1fe create test2.txt
Repo after event 11 (?s ago). Press 'h' for help, 'q' to quit.
Delete branch test1 at 62fc20d2 create test1.txt

O f777ecc9 (master) create initial.txt
|\\
| x 62fc20d2 (test1) create test1.txt
|
@ fe65c1fe create test2.txt
Repo after event 10 (?s ago). Press 'h' for help, 'q' to quit.
Hide commit 62fc20d2 create test1.txt

O f777ecc9 (master) create initial.txt
|\\
| o 62fc20d2 (test1) create test1.txt
|
@ fe65c1fe create test2.txt
Repo after event 9 (?s ago). Press 'h' for help, 'q' to quit.
Commit fe65c1fe create test2.txt

Will apply these actions:
1. Create branch test1 at 62fc20d2 create test1.txt
2. Unhide commit 62fc20d2 create test1.txt
Applied 2 inverse events.
""",
        )

    with io.StringIO() as out, io.StringIO() as err, mock_time_difference(), mock_keypresses(
        ["p", readchar.key.ENTER]
    ), mock_inputs(
        ["y"]
    ):
        assert undo(out=out, err=err, git_executable=git.git_executable) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
O f777ecc9 (master) create initial.txt
|\\
| o 62fc20d2 create test1.txt
|
@ fe65c1fe create test2.txt
Repo after event 12 (?s ago). Press 'h' for help, 'q' to quit.
Unhide commit 62fc20d2 create test1.txt

O f777ecc9 (master) create initial.txt
|
@ fe65c1fe create test2.txt
Repo after event 11 (?s ago). Press 'h' for help, 'q' to quit.
Delete branch test1 at 62fc20d2 create test1.txt

Will apply these actions:
1. Hide commit 62fc20d2 create test1.txt
Applied 1 inverse event.
""",
        )


@pytest.mark.parametrize(
    ("input", "expected"),
    [
        (
            [
                RefUpdateEvent(
                    timestamp=0.0,
                    ref_name="HEAD",
                    old_ref="1",
                    new_ref="2",
                    message=None,
                ),
                RefUpdateEvent(
                    timestamp=0.0,
                    ref_name="HEAD",
                    old_ref="1",
                    new_ref="3",
                    message=None,
                ),
            ],
            [
                RefUpdateEvent(
                    timestamp=0.0,
                    ref_name="HEAD",
                    old_ref="1",
                    new_ref="3",
                    message=None,
                )
            ],
        )
    ],
)
def test_optimize_inverse_events(input: List[Event], expected: List[Event]) -> None:
    actual = _optimize_inverse_events(input)
    assert actual == expected


def test_undo_move_refs(git: Git) -> None:
    git.requires_git29()

    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)

    with capture("out") as out, capture(
        "err"
    ) as err, mock_time_difference(), mock_keypresses(
        ["p", "p", "p", readchar.key.ENTER]
    ), mock_inputs(
        ["y"]
    ):
        assert (
            undo(out=out.stream, err=err.stream, git_executable=git.git_executable) == 0
        )
        compare(
            actual=out.getvalue(),
            expected=f"""\
:
@ 96d1c37a (master) create test2.txt
Repo after event 6 (?s ago). Press 'h' for help, 'q' to quit.
Commit 96d1c37a create test2.txt

:
@ 96d1c37a (master) create test2.txt
Repo after event 5 (?s ago). Press 'h' for help, 'q' to quit.
Move branch master from 62fc20d2 create test1.txt
                     to 96d1c37a create test2.txt
:
O 62fc20d2 (master) create test1.txt
|
@ 96d1c37a create test2.txt
Repo after event 4 (?s ago). Press 'h' for help, 'q' to quit.
Check out from 62fc20d2 create test1.txt
            to 96d1c37a create test2.txt
:
@ 62fc20d2 (master) create test1.txt
Repo after event 3 (?s ago). Press 'h' for help, 'q' to quit.
Commit 62fc20d2 create test1.txt

Will apply these actions:
1. Hide commit 96d1c37a create test2.txt
2. Move branch master from 96d1c37a create test2.txt
                        to 62fc20d2 create test1.txt
3. Check out from 96d1c37a create test2.txt
               to 62fc20d2 create test1.txt
branchless: {git.git_executable} checkout 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
A\ttest2.txt
Applied 3 inverse events.
""",
        )

    with capture("out") as out:
        assert smartlog(out=out.stream) == 0
        compare(
            actual=out.getvalue(),
            expected="""\
:
@ 62fc20d2 (master) create test1.txt
""",
        )
