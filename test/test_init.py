import stat

import pytest

from helpers import Git


def test_hook_installed(git: Git) -> None:
    git.init_repo()
    hook_path = git.path / ".git" / "hooks" / "post-commit"
    assert hook_path.exists()
    assert hook_path.stat().mode & stat.S_IXUSR
    assert hook_path.stat().mode & stat.S_IXGRP
    assert hook_path.stat().mode & stat.S_IXOTH


def test_alias_installed(git: Git) -> None:
    git.init_repo()
    assert (
        git.run("smartlog")
        == """\
@ f777ecc9 (master) create initial.txt
"""
    )
    assert (
        git.run("sl")
        == """\
@ f777ecc9 (master) create initial.txt
"""
    )


def test_old_git_version_warning(git: Git) -> None:
    git.init_repo()
    if git.get_version() >= (2, 29, 0):
        pytest.skip("Requires Git version earlier than v2.29")
    assert "requires Git v2.29" in git.run("branchless", ["init"]).replace("\n", " ")
