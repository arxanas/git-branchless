import io

import py
from branchless.hide import hide
from branchless.smartlog import smartlog

from helpers import git, git_commit_file, git_initial_commit, git_detach_head, compare


def test_hide_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_initial_commit()
        git_detach_head()
        git_commit_file(name="test1", time=1)
        git("checkout", ["master"])
        git_detach_head()
        git_commit_file(name="test2", time=2)

        with io.StringIO() as out:
            assert smartlog(out=out, show_old_commits=False) == 0
            compare(
                actual=out.getvalue(),
                expected="""\
* fe65c1fe create test2.txt
|
| o 62fc20d2 create test1.txt
|/
o f777ecc9 create initial.txt
""",
            )

        with io.StringIO() as out:
            assert hide(out=out, hash="62fc20d2") == 0
            compare(
                actual=out.getvalue(),
                expected="""\
Hid commit: 62fc20d2
To unhide this commit, run: git checkout 62fc20d2
""",
            )

        with io.StringIO() as out:
            assert smartlog(out=out, show_old_commits=False) == 0
            compare(
                actual=out.getvalue(),
                expected="""\
* fe65c1fe create test2.txt
|
o f777ecc9 create initial.txt
""",
            )
