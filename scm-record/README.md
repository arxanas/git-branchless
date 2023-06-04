# License

MIT OR Apache-2.0

# scm-record

`scm-record` is a UI component to interactively select changes to include in a commit. It's meant to be embedded in source control tooling.

You can think of this as an interactive replacement for `git add -p`, or a reimplementation of `hg crecord`/`hg commit -i`. Given a set of changes made by the user, this component presents them to the user and lets them select which of those changes should be staged for commit.

`scm-record` was originally developed as a supporting library for
[git-branchless](https://github.com/arxanas/git-branchless).

# scm-diff-editor

The `scm-diff-editor` binary is available when compiled with `--features scm-diff-editor`.

This is a standalone binary that uses the `scm-record` library as a front-end, and can be integrated directly into some source control systems:

- [Git](https://git-scm.org):
  - As a difftool (see [`git-difftool(1)`](https://git-scm.com/docs/git-difftool). Only supports viewing diffs, not editing them.
  - Via [git-branchless](https://github.com/arxanas/git-branchless), whose `git record -i` command lets you interactively select and commit changes.
- [Mercurial](https://www.mercurial-scm.org/): via [the `extdiff` extension](https://wiki.mercurial-scm.org/ExtdiffExtension). Only supports viewing diffs, not editing them.
- [Jujutsu](https://github.com/martinvonz/jj): via [the `ui.diff-editor` option](https://github.com/martinvonz/jj/blob/main/docs/config.md#editing-diffs).

# Integration with other projects

Here's some projects that don't use `scm-record`, but could benefit from integration with it (with your contribution):

- [Sapling](https://sapling-scm.com/).
- [Stacked Git](https://stacked-git.github.io/)
- [Pijul](https://pijul.org/)
- [gitoxide/ein](https://github.com/Byron/gitoxide)
- [gitui](https://github.com/extrawurst/gitui)
- [Game of Trees](https://gameoftrees.org/)

# Feature wishlist

Here are some features in the UI which are not yet implemented:

- Jump to next/previous element of same kind.
- Edit one side of the diff in an editor.
- Commit message entry.
- Multi-way split UI to split a commit into more than 2 commits.
- Support for use as a mergetool.
- Commands to select ours/theirs for diffs representing merge conflicts.
