"""Callbacks for Git hooks.

Git uses "hooks" to run user-defined scripts after certain events. We
extensively use these hooks to track user activity and e.g. decide if a
commit should be considered "hidden".

The hooks are installed by the `branchless init` command. This module
contains the implementations for the hooks.
"""
from .rust import (
    py_hook_post_checkout,
    py_hook_post_commit,
    py_hook_post_rewrite,
    py_hook_reference_transaction,
)

hook_post_rewrite = py_hook_post_rewrite
hook_post_checkout = py_hook_post_checkout
hook_post_commit = py_hook_post_commit
hook_reference_transaction = py_hook_reference_transaction
