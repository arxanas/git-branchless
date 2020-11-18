import io

import py
from branchless.smartlog import smartlog

from helpers import compare, git, git_init_repo, git_commit_file


def test_multiple_branches_for_same_commit(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git("branch", ["abc"])
        git("branch", ["xyz"])

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


def test_differential_revision_provider(tmpdir: py.path.local) -> None:
    with tmpdir.as_cwd():
        git_init_repo()
        git_commit_file(name="name1", time=1)
        git(
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
