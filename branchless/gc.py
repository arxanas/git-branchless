"""Deal with Git's garbage collection mechanism.

Git treats a commit as unreachable if there are no references that point to
it or one of its descendants. However, the branchless workflow oftentimes
involves keeping such commits reachable until the user has explicitly hidden
them.

This module is responsible for adding extra references to Git, so that Git's
garbage collection doesn't collect commits which branchless thinks are still
visible.
"""
from typing import TextIO

import pygit2

from . import get_branch_oid_to_names, get_head_oid, get_main_branch_oid, get_repo
from .db import make_db_for_repo
from .eventlog import EventLogDb, EventReplayer
from .graph import CommitGraph, make_graph
from .mergebase import MergeBaseDb


def is_gc_ref(ref_name: str) -> bool:
    """Determine whether a given reference is used to keep a commit alive.

    Args:
      ref_name: The name of the reference.

    Returns:
      Whether or not the given reference is used internally to keep the
      commit alive, so that it's not collected by Git's garbage collection
      mechanism.
    """
    return ref_name.startswith("refs/branchless/")


def mark_commit_reachable(repo: pygit2.Repository, commit_oid: pygit2.Oid) -> None:
    """Mark a commit as reachable.

    Once marked as reachable, the commit won't be collected by Git's garbage
    collection mechanism until first garbage-collected by branchless itself.

    Args:
      repo: The Git repository.
      commit_oid: The commit OID to mark as reachable.
    """
    ref_name = f"refs/branchless/{commit_oid.hex}"
    assert pygit2.reference_is_valid_name(ref_name)
    repo.references.create(name=ref_name, target=commit_oid, force=True)


def _garbage_collect(repo: pygit2.Repository, graph: CommitGraph) -> None:
    for ref_name in repo.references:
        ref = repo.references[ref_name]
        if is_gc_ref(ref_name) and ref.resolve().target.hex not in graph:
            repo.references.delete(ref_name)


def gc(*, out: TextIO) -> None:
    """Run branchless's garbage collection.

    Args:
      out: The output stream to write to.
    """
    repo = get_repo()
    db = make_db_for_repo(repo)
    event_log_db = EventLogDb(db)
    merge_base_db = MergeBaseDb(db)
    event_replayer = EventReplayer.from_event_log_db(event_log_db)
    head_oid = get_head_oid(repo)
    main_branch_oid = get_main_branch_oid(repo)
    branch_oid_to_names = get_branch_oid_to_names(repo)
    graph = make_graph(
        repo=repo,
        merge_base_db=merge_base_db,
        event_replayer=event_replayer,
        head_oid=head_oid.hex,
        main_branch_oid=main_branch_oid,
        branch_oids=set(branch_oid_to_names),
        hide_commits=True,
    )

    out.write("Garbage collecting...\n")
    _garbage_collect(repo=repo, graph=graph)
