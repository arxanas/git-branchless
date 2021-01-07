from typing import Dict

from . import OidStr
from .rust import PyNode as Node
from .rust import py_find_path_to_merge_base as find_path_to_merge_base
from .rust import py_make_graph as make_graph

CommitGraph = Dict[OidStr, Node]
"""Graph of commits that the user is working on."""

# For flake8.
_ = (find_path_to_merge_base, make_graph)
