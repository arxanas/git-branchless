"""Display a graph of commits that the user has worked on recently.

The set of commits that are still being worked on is inferred from the
ref-log; see the `reflog` module.
"""
from .rust import py_render_graph, py_smartlog

render_graph = py_render_graph
smartlog = py_smartlog
