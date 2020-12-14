from typing import Iterator

import py
import pytest
from _pytest.config.argparsing import Parser
from _pytest.fixtures import SubRequest

from helpers import Git


def pytest_addoption(parser: Parser) -> None:
    parser.addoption(
        "--git-path",
        type=str,
        default=None,
        help="The path to the Git executable to use. If not provided, uses the system Git.",
    )


@pytest.fixture
def git_executable(request: SubRequest) -> str:
    git_executable: str = request.config.getoption("--git-path")
    if git_executable is not None:
        return git_executable
    else:
        return "git"


@pytest.fixture
def git(tmpdir: py.path.local, git_executable: str) -> Iterator[Git]:
    with tmpdir.as_cwd():
        yield Git(tmpdir, git_executable=git_executable)
