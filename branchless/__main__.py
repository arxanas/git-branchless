import argparse
import logging
import sys
from typing import List, TextIO

from .debug import debug_ref_log_entry
from .smartlog import smartlog


def main(argv: List[str], *, out: TextIO) -> None:
    logging.basicConfig(level=logging.DEBUG)

    parser = argparse.ArgumentParser(prog="branchless", add_help=False)
    parser.add_argument(
        "-h", "--help", action="store_true", help="show this help message and exit"
    )
    subparsers = parser.add_subparsers(
        dest="subcommand",
    )
    smartlog_parser = subparsers.add_parser(
        "smartlog",
        aliases=["sl"],
        help=smartlog.__doc__,
    )
    smartlog_parser.add_argument(
        "--show-old", action="store_true", help="Show old commits (hidden by default)."
    )
    hide_parser = subparsers.add_parser("hide", help="hide a commit from the smartlog")
    hide_parser.add_argument("commit", type=str, help="The commit hash to hide.")
    debug_ref_log_entry_parser = subparsers.add_parser(
        "debug-ref-log-entry", help=debug_ref_log_entry.__doc__
    )
    debug_ref_log_entry_parser.add_argument("hash", type=str)
    args = parser.parse_args(argv)

    if args.help:
        parser.print_help(file=out)
    elif args.subcommand in ["smartlog", "sl"]:
        smartlog(out=out, show_old_commits=args.show_old)
    elif args.subcommand == "debug-ref-log-entry":
        debug_ref_log_entry(out=out, hash=args.hash)
    elif args.subcommand == "hide":
        hide(out=out, hash=args.hash)
    else:
        parser.print_usage(file=out)
        sys.exit(1)


if __name__ == "__main__":
    main(sys.argv[1:], out=sys.stdout)
