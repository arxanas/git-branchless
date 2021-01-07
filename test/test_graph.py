import contextlib
import functools
from typing import Iterator, List, Union

import pygit2
import pytest

from branchless import get_repo
from branchless.db import make_db_for_repo
from branchless.graph import find_path_to_merge_base
from branchless.mergebase import MergeBaseDb
from helpers import Git


@contextlib.contextmanager
def spy_repository_getitem() -> Iterator[List[str]]:
    seen_keys = []
    original_getitem = pygit2.Repository.__getitem__

    @functools.wraps(original_getitem)
    def getitem_spy(
        self: pygit2.Repository, key: Union[pygit2.Oid, str]
    ) -> pygit2.Commit:
        seen_keys.append(str(key))
        return original_getitem(self, key)

    try:
        pygit2.Repository.__getitem__ = getitem_spy  # type: ignore[assignment]
        # NOTE: deliberately yielding mutable list.
        yield seen_keys
    finally:
        pygit2.Repository.__getitem__ = original_getitem  # type: ignore[assignment]


# Must port to Rust.
@pytest.mark.xfail
def test_find_path_to_merge_base_stop_early(git: Git) -> None:
    git.init_repo()
    git.commit_file(name="test1", time=1)
    git.commit_file(name="test2", time=2)
    git.detach_head()
    git.commit_file(name="test3", time=3)

    repo = get_repo()
    db = make_db_for_repo(repo)
    merge_base_db = MergeBaseDb(db)
    test1_oid = repo["62fc20d2a290daea0d52bdc2ed2ad4be6491010e"].oid
    test2_oid = repo["96d1c37a3d4363611c49f7e52186e189a04c531f"].oid
    test3_oid = repo["70deb1e28791d8e7dd5a1f0c871a51b91282562f"].oid
    with spy_repository_getitem() as seen_oids:
        # Since `test3` is a descendant of `test2`, we will never find `test3`
        # by traversing the parents of `test2`. This test verifies that we stop
        # early by hitting the merge-base, rather than attempting to traverse
        # the entire repository history.
        path = find_path_to_merge_base(
            repo=repo,
            merge_base_db=merge_base_db,
            target_oid=test3_oid,
            commit_oid=test2_oid,
        )
        assert path is None

        assert test2_oid.hex in seen_oids
        assert test3_oid.hex not in seen_oids
        assert test1_oid.hex not in seen_oids
