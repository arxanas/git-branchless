from typing import AbstractSet, Iterator, List, Mapping, Optional, Union


class Oid:
    hex: str


class Commit:
    oid: Oid
    message: str
    commit_time: int
    commit_time_offset: int
    parents: List["Commit"]


class RefLogEntry:
    oid_old: Oid
    oid_new: Oid
    message: str


class ResolvedReference:
    target: Oid


class Reference:
    target: Union[Oid, str]

    def resolve(self) -> "ResolvedReference":
        ...

    def log(self) -> Iterator[RefLogEntry]:
        ...

    def set_target(
        self, target: Union[Oid, str], message: Optional[str] = None
    ) -> None:
        ...


class Branch:
    target: Oid


class WalkOption:
    pass


WalkOptions = AbstractSet[WalkOption]
GIT_SORT_TOPOLOGICAL: WalkOptions


class Repository:
    path: str
    """Path of the `.git` directory for the repository."""

    references: Mapping[Union[Oid, str], Reference]
    branches: Mapping[str, Branch]

    def __init__(self, path: str) -> None:
        ...

    def __getitem__(self, oid: Union[Oid, str]) -> Commit:
        ...

    def __contains__(self, oid: Union[Oid, str]) -> bool:
        pass

    def merge_base(self, lhs: Oid, rhs: Oid) -> Optional[Oid]:
        ...

    def walk(self, oid: Oid, options: WalkOptions) -> Iterator[Commit]:
        ...


def discover_repository(path: str) -> Optional[str]:
    ...
