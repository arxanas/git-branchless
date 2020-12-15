"""Allows undoing to a previous state of the repo.

This is accomplished by finding the events that have happened since a certain
time and inverting them.
"""
import time
from typing import List, TextIO

import colorama
import pygit2
import readchar

from . import OidStr, get_repo, run_git
from .db import make_db_for_repo
from .eventlog import (
    CommitEvent,
    Event,
    EventLogDb,
    EventReplayer,
    HideEvent,
    RefUpdateEvent,
    RewriteEvent,
    UnhideEvent,
)
from .formatting import Glyphs, make_glyphs, pluralize
from .graph import make_graph
from .mergebase import MergeBaseDb
from .metadata import (
    BranchesProvider,
    CommitMessageProvider,
    CommitOidProvider,
    DifferentialRevisionProvider,
    RelativeTimeProvider,
    render_commit_metadata,
)
from .smartlog import render_graph


def _render_ref_name(ref_name: str) -> str:
    if ref_name.startswith("refs/heads/"):
        return "branch " + ref_name[len("refs/heads/") :]
    else:
        return "ref " + ref_name


def _describe_event(
    glyphs: Glyphs, repo: pygit2.Repository, event: Event, now: int
) -> str:
    def render_commit(oid: OidStr) -> str:
        try:
            commit = repo[oid]
        except KeyError:
            return f"<unavailable: {oid} (possibly GC'ed)>"
        return render_commit_metadata(
            glyphs=glyphs,
            commit=commit,
            commit_metadata_providers=[
                CommitOidProvider(glyphs=glyphs, use_color=True),
                CommitMessageProvider(),
            ],
        )

    if isinstance(event, CommitEvent):
        return "Commit {}\n".format(render_commit(event.commit_oid))
    elif isinstance(event, HideEvent):
        return "Hide commit {}\n".format(render_commit(event.commit_oid))
    elif isinstance(event, UnhideEvent):
        return "Unhide commit {}\n".format(render_commit(event.commit_oid))
    elif isinstance(event, RefUpdateEvent):
        if event.ref_name == "HEAD":
            assert (
                event.new_ref is not None
            ), f"New ref was None for HEAD event: {event!r}"
            if event.old_ref is not None:
                return """
Check out from {}
            to {}
""".strip().format(
                    render_commit(event.old_ref), render_commit(event.new_ref)
                )
            else:
                # Not sure if this can happen (when a repo is created, maybe?).
                return """\
Check out to {}
""".strip().format(
                    render_commit(event.new_ref)
                )

        # Occasionally can happen with certain rebases, such as with the ref
        # `CHERRY_PICK_HEAD`. Should have been filtered out by the event
        # replayer by now.
        elif event.old_ref is None and event.new_ref is None:  # pragma: no cover
            ref_name = _render_ref_name(event.ref_name)  # pragma: no cover
            return """\
Empty event for {}
This event should not appear. This is a (benign) bug -- please report it.
""".format(
                ref_name
            )  # pragma: no cover

        elif event.old_ref is None:
            assert event.new_ref is not None
            ref_name = _render_ref_name(event.ref_name)
            return """\
Create {} at {}
""".format(
                ref_name, render_commit(event.new_ref)
            )

        elif event.new_ref is None:
            assert event.old_ref is not None
            ref_name = _render_ref_name(event.ref_name)
            return """\
Delete {} at {}
""".format(
                ref_name, render_commit(event.old_ref)
            )

        else:
            assert event.old_ref is not None
            assert event.new_ref is not None
            ref_name = _render_ref_name(event.ref_name)
            return """
Move {} from {}
     {}   to {}
""".strip().format(
                ref_name,
                render_commit(event.old_ref),
                " " * (len(ref_name)),
                render_commit(event.new_ref),
            )
    elif isinstance(event, RewriteEvent):
        return """
Rewrite commit {}
            as {}
""".strip().format(
            render_commit(event.old_commit_oid),
            render_commit(event.new_commit_oid),
        )


def _inverse_event(now: float, event: Event) -> Event:
    if isinstance(event, (CommitEvent, UnhideEvent)):
        return HideEvent(timestamp=now, commit_oid=event.commit_oid)
    elif isinstance(event, HideEvent):
        return UnhideEvent(timestamp=now, commit_oid=event.commit_oid)
    elif isinstance(event, RewriteEvent):
        return RewriteEvent(
            timestamp=now,
            old_commit_oid=event.new_commit_oid,
            new_commit_oid=event.old_commit_oid,
        )
    elif isinstance(event, RefUpdateEvent):
        return RefUpdateEvent(
            timestamp=now,
            ref_name=event.ref_name,
            old_ref=event.new_ref,
            new_ref=event.old_ref,
            message=None,
        )


def _describe_inverse_event(event: Event) -> str:
    if isinstance(event, CommitEvent):
        return "Commit {}".format(event.commit_oid)
    elif isinstance(event, HideEvent):
        return "Hide {}".format(event.commit_oid)
    elif isinstance(event, UnhideEvent):
        return "Unhide {}".format(event.commit_oid)
    elif isinstance(event, RewriteEvent):
        return "Rewrite {} to {}".format(event.old_commit_oid, event.new_commit_oid)
    elif isinstance(event, RefUpdateEvent):
        return "Update {} from {} to {}".format(
            event.ref_name, event.old_ref, event.new_ref
        )


def _select_past_event(
    out: TextIO,
    glyphs: Glyphs,
    repo: pygit2.Repository,
    merge_base_db: MergeBaseDb,
    event_replayer: EventReplayer,
    now: int,
) -> bool:
    while True:
        out.write(glyphs.terminal_clear_screen)
        event_before_cursor = event_replayer.get_event_before_cursor()
        head_oid = event_replayer.get_cursor_head_oid()
        main_branch_oid = event_replayer.get_cursor_main_branch_oid(repo)
        branch_oid_to_names = event_replayer.get_cursor_branch_oid_to_names(repo)

        graph = make_graph(
            repo=repo,
            merge_base_db=merge_base_db,
            event_replayer=event_replayer,
            head_oid=head_oid,
            main_branch_oid=main_branch_oid,
            branch_oids=set(branch_oid_to_names),
            hide_commits=True,
        )
        render_graph(
            out=out,
            glyphs=glyphs,
            repo=repo,
            merge_base_db=merge_base_db,
            graph=graph,
            head_oid=head_oid,
            commit_metadata_providers=[
                CommitOidProvider(glyphs=glyphs, use_color=True),
                RelativeTimeProvider(glyphs=glyphs, repo=repo, now=int(time.time())),
                BranchesProvider(
                    glyphs=glyphs, repo=repo, branch_oid_to_names=branch_oid_to_names
                ),
                DifferentialRevisionProvider(glyphs=glyphs, repo=repo),
                CommitMessageProvider(),
            ],
        )
        if event_before_cursor is None:
            out.write("There are no previous available events.\n")
        else:
            (event_id, event) = event_before_cursor
            event_description = _describe_event(
                glyphs=glyphs, repo=repo, event=event, now=now
            )
            relative_time = RelativeTimeProvider.describe_time_delta(
                now=now, previous_time=int(event.timestamp)
            )
            out.write(
                f"Repo after event {event_id} ({relative_time} ago). Press 'h' for help, 'q' to quit.\n{event_description}\n"
            )

        while True:
            command = readchar.readkey()
            if command in ["n", "N", readchar.key.RIGHT]:
                event_replayer.advance_cursor(1)
                break
            elif command in ["p", "P", readchar.key.LEFT]:
                event_replayer.advance_cursor(-1)
                break
            elif command in ["g", "G"]:
                event_num_str = input("Enter event ID to jump to: ")
                try:
                    event_num = int(event_num_str)
                    event_replayer.set_cursor(event_num)
                    break
                except ValueError:
                    out.write(f"Invalid event ID: {event_num_str}\n")
            elif command in ["q", "Q", readchar.key.CTRL_C]:
                return False
            elif command in ["h", "H", "?"]:
                title = glyphs.style(style=colorama.Style.BRIGHT, message="HOW TO USE")
                out.write(
                    f"""\
{title}
Use `git undo` to view and revert to previous states of the repository.

h/?: Show this help.
q: Quit.
p/n or <left>/<right>: View next/previous state.
g: Go to a provided event ID.
<enter>: Revert the repository to the given state (requires confirmation).

You can also copy a commit hash from the past and manually run `git unhide`
or `git rebase` on it.
"""
                )
            elif command in [readchar.key.ENTER, readchar.key.CR, readchar.key.LF]:
                return True
    return False


def _optimize_inverse_events(events: List[Event]) -> List[Event]:
    optimized_events: List[Event] = []
    seen_checkout = False
    for event in reversed(events):
        if isinstance(event, RefUpdateEvent) and event.ref_name == "HEAD":
            if seen_checkout:
                continue
            else:
                seen_checkout = True
                optimized_events.append(event)
        else:
            optimized_events.append(event)
    return list(reversed(optimized_events))


def _undo_events(
    out: TextIO,
    err: TextIO,
    glyphs: Glyphs,
    repo: pygit2.Repository,
    git_executable: str,
    event_log_db: EventLogDb,
    event_replayer: EventReplayer,
) -> int:
    out.write(glyphs.terminal_clear_screen)
    now = int(time.time())
    events_to_undo = event_replayer.get_events_since_cursor()
    inverse_events = [
        _inverse_event(now=now, event=event)
        for event in reversed(events_to_undo)
        if not (
            isinstance(event, RefUpdateEvent)
            and event.ref_name == "HEAD"
            and event.old_ref is None
        )
    ]
    inverse_events = _optimize_inverse_events(inverse_events)
    if not inverse_events:
        out.write("No undo actions to apply, exiting.\n")
        return 0

    out.write("Will apply these actions:\n")
    for i, inverse_event in enumerate(inverse_events, start=1):
        num_header = f"{i}. "
        out.write(num_header)
        for j, line in enumerate(
            _describe_event(
                glyphs=glyphs, repo=repo, event=inverse_event, now=now
            ).splitlines()
        ):
            if j != 0:
                out.write(" " * len(num_header))
            out.write(line)
            out.write("\n")

    while True:
        confirmation = input("Confirm? [yN] ")
        if confirmation in ["y", "Y"]:
            for event in inverse_events:
                if isinstance(event, RefUpdateEvent):
                    if event.ref_name == "HEAD":
                        # Most likely the user wanted to perform an actual
                        # checkout in this case, rather than just update `HEAD`
                        # (and be left with a dirty working copy). The `Git`
                        # command will update the event log appropriately, as
                        # it will invoke our hooks.
                        assert event.new_ref is not None
                        run_git(
                            out=out,
                            err=err,
                            git_executable=git_executable,
                            args=["checkout", event.new_ref],
                        )
                    elif event.old_ref is None and event.new_ref is None:
                        # Do nothing.
                        pass
                    elif event.new_ref is None:
                        assert event.old_ref is not None
                        try:
                            repo.references.delete(event.ref_name)
                        except KeyError:
                            out.write(
                                f"Reference {event.ref_name} did not exist, not deleting it.\n"
                            )
                    else:
                        # Create or update the given reference.
                        repo.references.create(
                            event.ref_name, event.new_ref, force=True
                        )
                else:
                    event_log_db.add_events([event])

            num_inverse_events = pluralize(
                amount=len(inverse_events),
                singular="inverse event",
                plural="inverse events",
            )
            out.write(f"Applied {num_inverse_events}.\n")
            return 0
        elif confirmation in ["n", "N", "q", "Q"]:
            out.write("Aborted.\n")
            return 1


def undo(out: TextIO, err: TextIO, git_executable: str) -> int:
    """Restore the repository to a previous state interactively.

    Args:
      out: The output stream to write to.
      err: The error stream to write to.
      git_executable: The path to the `git` executable on disk.

    Returns:
      Exit code (0 denotes successful exit).
    """
    now = int(time.time())
    glyphs = make_glyphs(out)
    repo = get_repo()
    db = make_db_for_repo(repo)
    event_log_db = EventLogDb(db)
    merge_base_db = MergeBaseDb(db)
    event_replayer = EventReplayer.from_event_log_db(event_log_db)

    if not _select_past_event(
        out=out,
        glyphs=glyphs,
        repo=repo,
        merge_base_db=merge_base_db,
        event_replayer=event_replayer,
        now=now,
    ):
        return 0

    return _undo_events(
        out=out,
        err=err,
        glyphs=glyphs,
        repo=repo,
        git_executable=git_executable,
        event_log_db=event_log_db,
        event_replayer=event_replayer,
    )
