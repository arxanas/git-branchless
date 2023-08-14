# git-record

Supporting library for
[git-branchless](https://github.com/arxanas/git-branchless).

**Deprecation notice**: This library is no longer used. It was replaced by [`scm-record`](https://crates.io/crates/scm-record). Originally, this was a UI component to interactively select changes to include in a commit, meant to be embedded in source control tooling.

- If you want a replacement for `git add -p`, in the style of `hg crecord`/`hg commit -i`, you can try the `git record` command from [git-branchless](https://github.com/arxanas/git-branchless).
- If you want a TUI tool usable as a Git difftool (see [`git-difftool(1)`](https://git-scm.com/docs/git-difftool)), you can try the `git branchless difftool` command from [git-branchless](https://github.com/arxanas/git-branchless)
