from typing import TextIO

from .rust import py_find_abandoned_children, py_restack
from .smartlog import smartlog

find_abandoned_children = py_find_abandoned_children


def restack(*, out: TextIO, err: TextIO, git_executable: str) -> int:
    """Restack all abandoned commits.

    Args:
      out: The output stream to write to.
      err: The error stream to write to.
      git_executable: The path to the `git` executable on disk.
      preserve_timestamps: Whether or not to use the original commit time for
        rebased commits, rather than the current time.

    Returns:
      Exit code (0 denotes successful exit).
    """
    result = py_restack(
        out=out,
        err=err,
        git_executable=git_executable,
    )
    if result != 0:
        return result

    # TODO: `py_restack` should also display smartlog.
    return smartlog(out=out)
