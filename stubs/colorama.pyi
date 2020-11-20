import enum

def init() -> None:
    ...


class Fore(str, enum.Enum):
    BLACK: str
    RED: str
    GREEN: str
    YELLOW: str
    BLUE: str
    MAGENTA: str
    CYAN: str
    WHITE: str
    RESET: str


class Back(str, enum.Enum):
    BLACK: str
    RED: str
    GREEN: str
    YELLOW: str
    BLUE: str
    MAGENTA: str
    CYAN: str
    WHITE: str
    RESET: str


class Style(str, enum.Enum):
    DIM: str
    NORMAL: str
    BRIGHT: str
    RESET_ALL: str
