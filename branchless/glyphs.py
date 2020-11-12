from typing import TextIO

import colorama
from typing_extensions import Protocol


class Glyphs(Protocol):
    line: str
    line_with_offshoot: str
    vertical_ellipsis: str
    slash: str
    commit: str
    commit_head: str

    def color_fg(self, color: colorama.Fore, message: str) -> str:
        ...


class TextGlyphs:
    line = "|"
    line_with_offshoot = line
    vertical_ellipsis = ":"
    slash = "/"
    commit = "o"
    commit_head = "*"

    def color_fg(self, color: colorama.Fore, message: str) -> str:
        return message


class PrettyGlyphs:
    line = "┃"
    line_with_offshoot = "┣"
    vertical_ellipsis = "⋮"
    slash = "━┛"
    commit = "◯"
    commit_head = "●"

    def __init__(self) -> None:
        colorama.init()

    def color_fg(self, color: colorama.Fore, message: str) -> str:
        return color + message + colorama.Fore.RESET


def make_glyphs(out: TextIO) -> Glyphs:
    if out.isatty():
        return PrettyGlyphs()
    else:
        return TextGlyphs()
