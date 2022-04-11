# git-record

Supporting library for
[git-branchless](https://github.com/arxanas/git-branchless).

This is a UI component to interactively select changes to include in a
commit. It's meant to be embedded in source control tooling.

You can think of this as an interactive replacement for `git add -p`, or a
reimplementation of `hg crecord`. Given a set of changes made by the user,
this component presents them to the user and lets them select which of those
changes should be staged for commit.

License: MIT OR Apache-2.0
