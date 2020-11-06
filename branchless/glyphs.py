from typing_extensions import Protocol


class Glyphs(Protocol):
    line: str
    line_with_offshoot: str
    vertical_ellipsis: str
    slash: str
    commit: str
    commit_head: str


class TextGlyphs:
    line = "|"
    line_with_offshoot = line
    vertical_ellipsis = ":"
    slash = "/"
    commit = "o"
    commit_head = "*"


class PrettyGlyphs:
    ENABLED = False

    line = "┃"
    line_with_offshoot = "┣"
    vertical_ellipsis = "⋮"
    slash = "━┛"
    commit = "◯"
    commit_head = "●"
