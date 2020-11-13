from typing import TextIO, cast
import string

import colorama
from typing_extensions import Protocol

import pygit2


class Glyphs(Protocol):
    line: str
    line_with_offshoot: str
    vertical_ellipsis: str
    slash: str
    commit: str
    commit_hidden: str
    commit_head: str
    commit_head_hidden: str

    def color_fg(self, color: colorama.Fore, message: str) -> str:
        ...

    def style(self, style: colorama.Style, message: str) -> str:
        ...


class TextGlyphs:
    line = "|"
    line_with_offshoot = line
    vertical_ellipsis = ":"
    slash = "/"
    commit = "o"
    commit_hidden = "x"
    commit_head = "*"
    commit_head_hidden = "%"

    def color_fg(self, color: colorama.Fore, message: str) -> str:
        return message

    def style(self, style: colorama.Style, message: str) -> str:
        return message


class PrettyGlyphs:
    line = "┃"
    line_with_offshoot = "┣"
    vertical_ellipsis = "⋮"
    slash = "━┛"
    commit = "◯"
    commit_hidden = "✕"
    commit_head = "●"
    commit_head_hidden = "⦻"

    def __init__(self) -> None:
        colorama.init()

    def color_fg(self, color: colorama.Fore, message: str) -> str:
        return color + message + colorama.Fore.RESET

    def style(self, style: colorama.Style, message: str) -> str:
        return style + message + colorama.Style.RESET_ALL


def make_glyphs(out: TextIO) -> Glyphs:
    if out.isatty():
        return PrettyGlyphs()
    else:
        return TextGlyphs()


class Formatter(string.Formatter):
    """Formatter with additional directives for commits, etc."""

    def format_field(
        self,
        value: object,
        format_spec: str,
    ) -> str:
        if format_spec == "oid":
            assert isinstance(value, pygit2.Oid)
            return f"{value!s:8.8}"
        elif format_spec == "commit":
            assert isinstance(value, pygit2.Commit)
            return value.message.split("\n", 1)[0]
        else:
            result = super().format_field(value, format_spec)
            return cast(str, result)
