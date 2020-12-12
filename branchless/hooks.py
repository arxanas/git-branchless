"""Callbacks for Git hooks.

Git uses "hooks" to run user-defined scripts after certain events. We
extensively use these hooks to track user activity and e.g. decide if a
commit should be considered "hidden".

The hooks are installed by the `branchless init` command. This module
contains the implementations for the hooks.
"""
import os.path
import sys
import time
from typing import Set, TextIO

import colorama
import pygit2

from . import get_branch_oid_to_names, get_head_oid, get_main_branch_oid, get_repo
from .db import make_db_for_repo
from .eventlog import (
    CommitEvent,
    EventLogDb,
    EventReplayer,
    RefUpdateEvent,
    RewriteEvent,
)
from .formatting import make_glyphs, pluralize
from .graph import make_graph
from .mergebase import MergeBaseDb
from .restack import find_abandoned_children


def _is_rebase_underway(repo: pygit2.Repository) -> bool:
    """Detect if an interactive rebase has started but not completed.

    Git will send us spurious `post-rewrite` events marked as `amend` during
    an interactive rebase, indicating that some of the commits have been
    rewritten as part of the rebase plan, but not all of them. This function
    attempts to detect when an interactive rebase is underway, and if the
    current `post-rewrite` event is spurious.

    There are two practical issues for users as a result of this Git behavior:

      * During an interactive rebase, we may see many "processing 1 rewritten
      commit" messages, and then a final "processing X rewritten commits"
      message once the rebase has concluded. This is potentially confusing
      for users, since the operation logically only rewrote the commits once,
      but we displayed the message multiple times.

      * During an interactive rebase, we may warn about abandoned commits, when
      the next operation in the rebase plan fixes up the abandoned commit.
      This can happen even if no conflict occurred and the rebase completed
      successfully without any user intervention.
    """
    for subdir in ["rebase-apply", "rebase-merge"]:
        rebase_info_dir = os.path.join(repo.path, subdir)
        if os.path.exists(rebase_info_dir):
            return True
    return False


def hook_post_rewrite(out: TextIO, rewrite_type: str) -> None:
    """Handle Git's post-rewrite hook.

    Args:
      out: Output stream to write to.
      rewrite_type: The type of rewrite. Currently one of "rebase" or
        "amend".
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

    message_rewritten_commits = pluralize(
        amount=len(events), singular="rewritten commit", plural="rewritten commits"
    )

    repo = get_repo()
    is_spurious_event = rewrite_type == "amend" and _is_rebase_underway(repo)
    if not is_spurious_event:
        out.write(f"branchless: processing {message_rewritten_commits}\n")

    db = make_db_for_repo(repo=repo)
    event_log_db = EventLogDb(db)
    event_log_db.add_events(events)

    warn_config_key = "branchless.restack.warnAbandoned"
    try:
        should_check_abandoned_commits = repo.config.get_bool(warn_config_key)
    except KeyError:
        should_check_abandoned_commits = True

    if should_check_abandoned_commits and not is_spurious_event:
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

        # Use a set for abandoned children to make sure we don't double-count
        # any children (possibly in the case of a merge commit).
        all_abandoned_children = set()
        all_abandoned_branches: Set[str] = set()
        for old_commit_oid in old_commits:
            abandoned_result = find_abandoned_children(
                graph=graph, event_replayer=event_replayer, oid=old_commit_oid
            )
            if abandoned_result is None:
                continue
            (_rewritten_oid, abandoned_children) = abandoned_result
            all_abandoned_children.update(abandoned_children)
            all_abandoned_branches.update(branch_oid_to_names.get(old_commit_oid, []))
        num_abandoned_children = len(all_abandoned_children)
        num_abandoned_branches = len(all_abandoned_branches)

        if num_abandoned_children > 0 or num_abandoned_branches > 0:
            glyphs = make_glyphs(out=out)
            warning_items = []
            if num_abandoned_children > 0:
                warning_items.append(
                    pluralize(
                        amount=num_abandoned_children,
                        singular="commit",
                        plural="commits",
                    )
                )
            if num_abandoned_branches > 0:
                abandoned_branches_count = pluralize(
                    amount=num_abandoned_branches,
                    singular="branch",
                    plural="branches",
                )
                abandoned_branches_list = ", ".join(sorted(all_abandoned_branches))
                warning_items.append(
                    f"{abandoned_branches_count} ({abandoned_branches_list})"
                )

            warning_message = " and ".join(warning_items)
            warning_message = glyphs.style(
                style=colorama.Style.BRIGHT,
                message=glyphs.color_fg(
                    color=colorama.Fore.YELLOW,
                    message=f"This operation abandoned {warning_message}!",
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
