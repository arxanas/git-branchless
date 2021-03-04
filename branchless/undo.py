"""Allows undoing to a previous state of the repo.

This is accomplished by finding the events that have happened since a certain
time and inverting them.
"""
from .rust import py_undo

undo = py_undo
