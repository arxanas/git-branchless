from typing import TextIO

from . import get_repo
from .formatting import Formatter, make_glyphs

CHECKOUT_REF_LOG_COMMAND = "branchless-checkout"
HIDE_REF_LOG_COMMAND = "branchless-hide"


def hide(*, out: TextIO, hash: str) -> None:
    glyphs = make_glyphs(out)
    formatter = Formatter()
    repo = get_repo()
    oid = repo[hash].oid

    head_ref = repo.references["HEAD"]
    current_target = head_ref.target
    try:
        head_ref.set_target(oid, CHECKOUT_REF_LOG_COMMAND)
    finally:
        head_ref.set_target(current_target, HIDE_REF_LOG_COMMAND)

    out.write(formatter.format("Hid commit: {oid:oid}\n", oid=oid))
    out.write(
        formatter.format(
            "To unhide this commit, run: git checkout {oid:oid}\n", oid=oid
        )
    )
