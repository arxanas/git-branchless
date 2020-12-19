import contextlib
import difflib
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Iterator, List, Optional, TextIO, cast

import py
import pygit2
import pytest

from branchless import GitVersion, parse_git_version_output

DUMMY_NAME = "Testy McTestface"
DUMMY_EMAIL = "test@example.com"
DUMMY_DATE = "Wed 29 Oct 12:34:56 2020 PDT"


class Git:
    def __init__(self, path: py.path.local, git_executable: str) -> None:
        self.path = path
        self.git_executable = git_executable

    def init_repo(self, make_initial_commit: bool = True) -> None:
        self.run("init")
        self.run("config", ["user.name", DUMMY_NAME])
        self.run("config", ["user.email", DUMMY_EMAIL])

        if make_initial_commit:
            self.commit_file(name="initial", time=0)

        python_path = Path(__file__).parent.parent
        self.run(
            "config",
            [
                "alias.branchless",
                f"!env PYTHONPATH={python_path} {sys.executable} -m branchless",
            ],
        )

        # Silence some log-spam.
        self.run("config", ["advice.detachedHead", "false"])

        # Non-deterministic metadata (depends on current time).
        self.run("config", ["branchless.commitMetadata.relativeTime", "false"])

        self.run("branchless", ["init"])

    def get_repo(self) -> pygit2.Repository:
        return pygit2.Repository(str(self.path))

    def run(
        self,
        command: str,
        args: Optional[List[str]] = None,
        time: int = 0,
        check: bool = True,
    ) -> str:
        if args is None:
            args = []
        args = [self.git_executable, command, *args]

        # Required for determinism, as these values will be baked into the commit
        # hash.
        date = f"{DUMMY_DATE} -{time:02d}00"
        env = {
            "GIT_AUTHOR_DATE": date,
            "GIT_COMMITTER_DATE": date,
            # Fake "editor" which accepts the default contents of any commit
            # messages. Usually, we can set this with `git commit -m`, but we have
            # no such option for things such as `git rebase`, which may call `git
            # commit` later as a part of their execution.
            "GIT_EDITOR": "true",
            # Should be set by `git` fixture.
            "PATH_TO_GIT": os.environ["PATH_TO_GIT"],
        }
        env.update({k: v for k, v in os.environ.items() if k.startswith("COV_")})

        result = subprocess.run(
            args,
            stdout=subprocess.PIPE,
            env=env,
            check=check,
        )
        return result.stdout.decode()

    def get_version(self) -> GitVersion:
        version_str = self.run("version")
        return parse_git_version_output(version_str)

    def requires_git29(self) -> None:
        version = self.get_version()
        if version < (2, 29, 0):
            version_str = ".".join(str(i) for i in version)
            pytest.skip(f"Requires Git v2.29 or above (current is: {version_str})")

    def commit_file(self, name: str, time: int, contents: Optional[str] = None) -> None:
        path = os.path.join(os.getcwd(), f"{name}.txt")
        with open(path, "w") as f:
            if contents is None:
                f.write(f"{name} contents\n")
            else:
                f.write(contents)
                f.write("\n")
        self.run("add", ["."])
        self.run("commit", ["-m", f"create {name}.txt"], time=time)

    def resolve_file(self, name: str, contents: str) -> None:
        path = os.path.join(os.getcwd(), f"{name}.txt")
        with open(path, "w") as f:
            f.write(contents)
            f.write("\n")
        self.run("add", [path])

    def detach_head(self) -> None:
        self.run("checkout", ["--detach", "HEAD"])


def _rstrip_lines(lines: str) -> List[str]:
    return [line.rstrip() + "\n" for line in lines.splitlines()]


def compare(actual: str, expected: str) -> None:
    actual_lines = _rstrip_lines(actual)
    expected_lines = _rstrip_lines(expected)

    sys.stdout.writelines(
        difflib.context_diff(
            expected_lines, actual_lines, fromfile="Expected", tofile="Actual", n=999
        )
    )
    assert actual == expected


class FileBasedCapture:
    """Wraps a `TextIO` by putting it in a temporary file.

    This is necessary for operations such as `subprocess.call`, which call
    `.fileno()` on the stream. This method is not available for in-memory
    `io.StringIO` instances.
    """

    def __init__(self, stream: TextIO) -> None:
        """Constructor.

        Args:
          stream: The handle to the underlying file to wrap.
        """
        self.stream = stream

    def getvalue(self) -> str:
        """Get the data that has been written to this stream so far.

        Must only be called once.
        """
        self.stream.seek(0)
        return self.stream.read()


@contextlib.contextmanager
def capture(debug_name: str) -> Iterator[FileBasedCapture]:
    with tempfile.NamedTemporaryFile("r+") as f:
        capture = FileBasedCapture(cast(TextIO, f))
        try:
            yield capture
        except Exception:
            print(f"Captured output for {debug_name}:")
            print(capture.getvalue())
            raise
