"""Callbacks for Git hooks.

Git uses "hooks" to run user-defined scripts after certain events. We
extensively use these hooks to track user activity and e.g. decide if a
commit should be considered "hidden".

The hooks are installed by the `branchless init` command. This module
contains the implementations for the hooks.
"""
import sys
import time
from typing import TextIO

import colorama

from . import get_branch_oid_to_names, get_head_oid, get_main_branch_oid, get_repo
from .db import make_db_for_repo
from .eventlog import (
    CommitEvent,
    EventLogDb,
    EventReplayer,
    RefUpdateEvent,
    RewriteEvent,
)
from .formatting import make_glyphs
from .graph import make_graph
from .mergebase import MergeBaseDb
from .restack import find_abandoned_children


def hook_post_rewrite(out: TextIO) -> None:
    """Handle Git's post-rewrite hook.

    Args:
      out: Output stream to write to.
    """
    timestamp = time.time()
    old_commits = []
    events = []
    for line in sys.stdin:
        line = line.strip()
        [old_ref, new_ref, *extras] = line.split(" ")
        old_commits.append(old_ref)
        events.append(
            RewriteEvent(
                timestamp=timestamp, old_commit_oid=old_ref, new_commit_oid=new_ref
            )
        )
    out.write(f"branchless: processing {len(events)} rewritten commit(s)\n")

    repo = get_repo()
    db = make_db_for_repo(repo=repo)
    event_log_db = EventLogDb(db)
    event_log_db.add_events(events)

    warn_config_key = "branchless.restack.warnAbandoned"
    try:
        should_check_abandoned_commits = repo.config.get_bool(warn_config_key)
    except KeyError:
        should_check_abandoned_commits = True

    if should_check_abandoned_commits:
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
            hide_commits=False,
        )

        # Make sure we don't double-count any children (possibly in the case of
        # a merge commit).
        all_abandoned_children = set()
        # Branches can only point to one OID, so we can just keep count.
        num_abandoned_branches = 0
        for old_commit_oid in old_commits:
            abandoned_result = find_abandoned_children(
                graph=graph, event_replayer=event_replayer, oid=old_commit_oid
            )
            if abandoned_result is None:
                continue
            (_rewritten_oid, abandoned_children) = abandoned_result
            all_abandoned_children.update(abandoned_children)
            num_abandoned_branches += len(branch_oid_to_names.get(old_commit_oid, []))
        num_abandoned_children = len(all_abandoned_children)

        if num_abandoned_children > 0 or num_abandoned_branches > 0:
            glyphs = make_glyphs(out=out)
            warning_message = glyphs.style(
                style=colorama.Style.BRIGHT,
                message=glyphs.color_fg(
                    color=colorama.Fore.YELLOW,
                    message=f"This operation abandoned {num_abandoned_children} commit(s) and {num_abandoned_branches} branch(es)!",
                ),
            )

            def command(s: str) -> str:
                return glyphs.style(style=colorama.Style.BRIGHT, message=s)

            git_smartlog = command("git smartlog")
            git_restack = command("git restack")
            git_hide = command("git hide [<commit>...]")
            git_undo = command("git undo")
            git_config = command(f"git config {warn_config_key} false")

            out.write(
                f"""\
branchless: {warning_message}
branchless: Consider running one of the following:
branchless:   - {git_restack}: re-apply the abandoned commits/branches
branchless:     (this is most likely what you want to do)
branchless:   - {git_smartlog}: assess the situation
branchless:   - {git_hide}: hide the commits from the smartlog
branchless:   - {git_undo}: undo the operation
branchless:   - {git_config}: suppress this message
"""
            )


def hook_post_checkout(
    out: TextIO, previous_head_ref: str, current_head_ref: str, is_branch_checkout: int
) -> None:
    """Handle Git's post-checkout hook.

    Args:
      out: Output stream to write to.
    """
    if is_branch_checkout == 0:
        return

    timestamp = time.time()
    out.write("branchless: processing checkout\n")

    repo = get_repo()
    db = make_db_for_repo(repo=repo)
    event_log_db = EventLogDb(db)
    event_log_db.add_events(
        [
            RefUpdateEvent(
                timestamp=timestamp,
                old_ref=previous_head_ref,
                new_ref=current_head_ref,
                ref_name="HEAD",
                message=None,
            )
        ]
    )


def hook_post_commit(out: TextIO) -> None:
    """Handle Git's post-commit hook.

    Args:
      out: Output stream to write to.
    """
    out.write("branchless: processing commit\n")

    repo = get_repo()
    db = make_db_for_repo(repo=repo)
    event_log_db = EventLogDb(db)

    commit_oid = repo.head.target
    timestamp = repo[commit_oid].commit_time
    event_log_db.add_events(
        [CommitEvent(timestamp=timestamp, commit_oid=commit_oid.hex)]
    )
