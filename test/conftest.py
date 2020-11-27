from typing import Iterator

import py
import pytest

from helpers import Git


@pytest.fixture
def git(tmpdir: py.path.local) -> Iterator[Git]:
    with tmpdir.as_cwd():
        yield Git(tmpdir)
