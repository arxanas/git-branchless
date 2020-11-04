from dataclasses import dataclass
from typing import List, Set, cast

import pygit2
from branchless.reflog import RefLogReplayer


@dataclass
class MyRefLogEntry:
    oid_old: str
    oid_new: str
    message: str


def process_ref_log(ref_log: str) -> RefLogReplayer:
    replayer = RefLogReplayer(cast(pygit2.Oid, "0"))
    for line in reversed(ref_log.strip().splitlines()):
        (oid_old, oid_new, message) = line.split(maxsplit=2)
        replayer.process(
            cast(
                pygit2.RefLogEntry,
                MyRefLogEntry(oid_old=oid_old, oid_new=oid_new, message=message),
            )
        )
    replayer.finish_processing()
    return replayer


def compare_visible_oids(replayer: RefLogReplayer, expected: Set[str]) -> None:
    actual = {str(i) for i in replayer.get_visible_oids()}
    assert actual == expected


def test_rebase() -> None:
    replayer = process_ref_log(
        """\
0 1 commit (initial): create initial.txt
1 1 checkout: moving from master to test1
1 2 commit: create test1.txt
2 1 checkout: moving from test1 to master
1 3 commit: create test2.txt
3 2 rebase (start): checkout test1
2 4 rebase (pick): create test2.txt
4 4 rebase (finish): returning to refs/heads/master
""",
    )

    # OID 3 should have been superseded by OID 4 by the "rebase (start)"
    # action.
    compare_visible_oids(replayer, {"1", "2", "4"})
