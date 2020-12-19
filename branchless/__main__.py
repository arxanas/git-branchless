"""Main entry-point."""
import argparse
import logging
import os
import sys
from typing import List, TextIO

from .gc import gc
from .hide import hide, unhide
from .hooks import (
    hook_post_checkout,
    hook_post_commit,
    hook_post_rewrite,
    hook_reference_transaction,
)
from .init import init
from .navigation import next, prev
from .restack import restack
from .smartlog import smartlog
from .undo import undo


def main(argv: List[str], *, out: TextIO, err: TextIO, git_executable: str) -> int:
    """Run the provided sub-command.

    Args:
      argv: List of command-line arguments (e.g. from `sys.argv`).
      out: Output stream to write to (may be a TTY).
      git_executable: The path to the `git` executable on disk.

    Returns:
      Exit code (0 denotes successful exit).
    """
    logging.basicConfig(level=logging.DEBUG)

    parser = argparse.ArgumentParser(prog="branchless", add_help=False)
    parser.add_argument(
        "-h", "--help", action="store_true", help="show this help message and exit"
    )
    subparsers = parser.add_subparsers(
        dest="subcommand",
    )

    # Command parsers.
    subparsers.add_parser(
        "init", help="Initialize the branchless workflow for this repository."
    )
    subparsers.add_parser(
        "smartlog",
        aliases=["sl"],
        help=smartlog.__doc__,
    )
    hide_parser = subparsers.add_parser("hide", help="Hide a commit from the smartlog.")
    hide_parser.add_argument(
        "hash", type=str, help="The commit hash to hide.", nargs="*"
    )
    hide_parser.add_argument(
        "-r",
        "--recursive",
        action="store_true",
        help="Hide all visible descendant commits as well.",
    )
    unhide_parser = subparsers.add_parser(
        "unhide", help="Unhide a previously-hidden commit from the smartlog."
    )
    unhide_parser.add_argument(
        "-r",
        "--recursive",
        action="store_true",
        help="Unhide all visible descendant commits as well.",
    )
    unhide_parser.add_argument(
        "hash", type=str, help="The commit hash to unhide.", nargs="*"
    )
    prev_parser = subparsers.add_parser("prev", help="Go to a previous commit.")
    prev_parser.add_argument(
        "num_commits", type=int, help="The number of commits backward to go.", nargs="?"
    )
    next_parser = subparsers.add_parser("next", help="Go to a later commit.")
    next_parser.add_argument(
        "num_commits", type=int, help="The number of commits forward to go.", nargs="?"
    )
    next_parser_towards_group = next_parser.add_mutually_exclusive_group()
    next_parser_towards_group.add_argument(
        "-o",
        "--oldest",
        help="When encountering multiple next commits, choose the oldest.",
        dest="towards",
        action="store_const",
        const="oldest",
    )
    next_parser_towards_group.add_argument(
        "-n",
        "--newest",
        help="When encountering multiple next commits, choose the newest.",
        dest="towards",
        action="store_const",
        const="newest",
    )
    subparsers.add_parser(
        "restack",
        help="Rebase abandoned commits onto their most up-to-date counterparts.",
    )
    subparsers.add_parser(
        "undo",
        help="Return to a past state of the repository.",
    )

    subparsers.add_parser("gc", help="Run internal garbage collection.")

    # Hook parsers.
    hook_post_rewrite_parser = subparsers.add_parser(
        "hook-post-rewrite", help="Internal use."
    )
    hook_post_rewrite_parser.add_argument("rewrite_type", type=str)
    hook_post_checkout_parser = subparsers.add_parser(
        "hook-post-checkout", help="Internal use."
    )
    hook_post_checkout_parser.add_argument("previous_commit", type=str)
    hook_post_checkout_parser.add_argument("current_commit", type=str)
    hook_post_checkout_parser.add_argument("is_branch_checkout", type=int)
    subparsers.add_parser("hook-pre-auto-gc", help="Internal use.")
    hook_reference_transaction_parser = subparsers.add_parser(
        "hook-reference-transaction", help="Internal use."
    )
    hook_reference_transaction_parser.add_argument("transaction_state", type=str)
    subparsers.add_parser("hook-post-commit", help="Internal use.")

    args = parser.parse_args(argv)

    if args.help:
        parser.print_help(file=out)
        return 0
    elif args.subcommand == "init":
        return init(out=out, git_executable=git_executable)
    elif args.subcommand in ["smartlog", "sl"]:
        return smartlog(out=out)
    elif args.subcommand == "hide":
        return hide(out=out, hashes=args.hash, recursive=args.recursive)
    elif args.subcommand == "unhide":
        return unhide(out=out, hashes=args.hash, recursive=args.recursive)
    elif args.subcommand == "prev":
        return prev(
            out=out,
            err=err,
            git_executable=git_executable,
            num_commits=args.num_commits,
        )
    elif args.subcommand == "next":
        return next(
            out=out,
            err=err,
            git_executable=git_executable,
            num_commits=args.num_commits,
            towards=args.towards,
        )
    elif args.subcommand == "restack":
        return restack(
            out=out, err=err, git_executable=git_executable, preserve_timestamps=False
        )
    elif args.subcommand == "undo":
        return undo(out=out, err=err, git_executable=git_executable)
    elif args.subcommand in ["gc", "hook-pre-auto-gc"]:
        gc(out=out)
        return 0
    elif args.subcommand == "hook-post-rewrite":
        hook_post_rewrite(out=out, rewrite_type=args.rewrite_type)
        return 0
    elif args.subcommand == "hook-post-checkout":
        hook_post_checkout(
            out=out,
            previous_head_ref=args.previous_commit,
            current_head_ref=args.current_commit,
            is_branch_checkout=args.is_branch_checkout,
        )
        return 0
    elif args.subcommand == "hook-post-commit":
        hook_post_commit(out=out)
        return 0
    elif args.subcommand == "hook-reference-transaction":
        hook_reference_transaction(out=out, transaction_state=args.transaction_state)
        return 0
    else:
        parser.print_usage(file=out)
        return 1


def entry_point() -> None:
    try:
        from pytest_cov.embed import init as pytest_cov_init

        pytest_cov_init()
    except ImportError:  # pragma: no cover
        pass

    # `PATH_TO_GIT` set in testing.
    git_executable = os.environ.get("PATH_TO_GIT", "git")

    sys.exit(
        main(
            sys.argv[1:], out=sys.stdout, err=sys.stderr, git_executable=git_executable
        )
    )


if __name__ == "__main__":
    entry_point()
