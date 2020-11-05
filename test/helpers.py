from typing import Optional, List

import os
import subprocess

GIT_PATH = "/opt/twitter_mde/bin/git"

DUMMY_NAME = "Testy McTestface"
DUMMY_EMAIL = "test@example.com"
DUMMY_DATE = "Wed 29 Oct 12:34:56 2020 PDT"


def git(
    command: str,
    args: Optional[List[str]] = None,
    time: int = 0,
) -> str:
    if args is None:
        args = []
    args = [GIT_PATH, command, *args]

    # Required for determinism, as these values will be baked into the commit
    # hash.
    date = f"{DUMMY_DATE} -{time:02d}00"
    env = {
        "GIT_AUTHOR_NAME": DUMMY_NAME,
        "GIT_AUTHOR_EMAIL": DUMMY_EMAIL,
        "GIT_AUTHOR_DATE": date,
        "GIT_COMMITTER_NAME": DUMMY_NAME,
        "GIT_COMMITTER_EMAIL": DUMMY_EMAIL,
        "GIT_COMMITTER_DATE": date,
        "GIT_EDITOR": "true",
    }

    result = subprocess.run(args, stdout=subprocess.PIPE, env=env)
    return result.stdout.decode()


def git_commit_file(name: str, time: int, contents: Optional[str] = None) -> None:
    path = os.path.join(os.getcwd(), f"{name}.txt")
    with open(path, "w") as f:
        if contents is None:
            f.write(f"{name} contents\n")
        else:
            f.write(contents)
            f.write("\n")
    git("add", ["."])
    git("commit", ["-m", f"create {name}.txt"], time=time)


def git_resolve_file(name: str, contents: str) -> None:
    path = os.path.join(os.getcwd(), f"{name}.txt")
    with open(path, "w") as f:
        f.write(contents)
        f.write("\n")
    git("add", [path])


def git_initial_commit() -> None:
    git("init")
    git_commit_file(name="initial", time=0)


def git_detach_head() -> None:
    git("checkout", ["--detach", "HEAD"])


def _rstrip_lines(lines: str) -> str:
    return "".join(line.rstrip() + "\n" for line in lines.splitlines())


def compare(actual: str, expected: str) -> None:
    actual = _rstrip_lines(actual)
    expected = _rstrip_lines(expected)
    print("Expected:")
    print(expected)
    print("Actual:")
    print(actual)
    assert actual == expected