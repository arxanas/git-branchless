"""Formatting and output helpers.

We try to handle both textual output and interactive output (output to a
"TTY"). In the case of interactive output, we render with prettier non-ASCII
characters and with colors, using shell-specific escape codes.
"""
import string
from typing import TextIO, cast

import colorama
import pygit2
from typing_extensions import Protocol


class Glyphs(Protocol):
    """Interface for glyphs to use for rendering the smartlog."""

    line: str
    line_with_offshoot: str
    vertical_ellipsis: str
    slash: str
    commit_visible: str
    commit_visible_head: str
    commit_hidden: str
    commit_hidden_head: str
    commit_master: str
    commit_master_head: str

    def color_fg(self, color: colorama.Fore, message: str) -> str:
        """Render the foreground (text) color for the given message.

        Args:
          color: The color to render the foreground as.
          message: The message to render.

        Returns:
          An updated message that potentially includes escape codes to render
          the color.
        """
        ...

    def style(self, style: colorama.Style, message: str) -> str:
        """Apply a certain style to the given message.

        Args:
          style: The style to apply.
          message: The message to render.

        Returns:
          An updated message that potentially includes escape codes to render
          the style.
        """
        ...


class TextGlyphs:
    """Glyphs used for output to a text file or non-TTY."""

    line = "|"
    line_with_offshoot = line
    vertical_ellipsis = ":"
    slash = "\\"
    commit_visible = "o"
    commit_visible_head = "@"
    commit_hidden = "x"
    commit_hidden_head = "%"
    commit_master = "O"
    commit_master_head = "@"

    def color_fg(self, color: colorama.Fore, message: str) -> str:
        return message

    def style(self, style: colorama.Style, message: str) -> str:
        return message


class PrettyGlyphs:
    """Glyphs used for output to a TTY."""

    line = "┃"
    line_with_offshoot = "┣"
    vertical_ellipsis = "⋮"
    slash = "━┓"
    commit_visible = "◯"
    commit_visible_head = "●"
    commit_hidden = "✕"
    commit_hidden_head = "⦻"
    commit_master = "◇"
    commit_master_head = "◆"

    def __init__(self) -> None:
        colorama.init()

    def color_fg(self, color: colorama.Fore, message: str) -> str:
        return color + message + colorama.Fore.RESET

    def style(self, style: colorama.Style, message: str) -> str:
        return style + message + colorama.Style.RESET_ALL


def make_glyphs(out: TextIO) -> Glyphs:
    """Make the `Glyphs` object appropriate for the provided output stream.

    Args:
      out: The output stream being written to.

    Returns:
      The `Glyphs` object.
    """
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
            return f"{value!s:8.8}"
        elif format_spec == "commit":
            assert isinstance(value, pygit2.Commit)
            return value.message.split("\n", 1)[0]
        else:
            result = super().format_field(value, format_spec)
            return cast(str, result)
