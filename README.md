<p align="center"><img width="147" height="147" src="https://user-images.githubusercontent.com/454057/144287756-8570ba1b-b9f1-46de-9236-ca17db246856.png" alt="git-branchless logo" /></p>

<h1 align="center">Branchless workflow for Git</h1>
<p align="center">(This suite of tools is 100% compatible with branches. If you think this is confusing, you can <a href="https://github.com/arxanas/git-branchless/discussions/284">suggest a new name here</a>.)</p>

<hr />

<table align="center">
  <tbody>
    <tr>
      <td align="center"><a href="https://github.com/arxanas/git-branchless/actions/workflows/linux.yml"><img alt="Linux build status" src="https://github.com/arxanas/git-branchless/actions/workflows/linux.yml/badge.svg" /></a></td>
      <td align="center"><a href="https://github.com/arxanas/git-branchless/actions/workflows/windows.yml"><img alt="Windows build status" src="https://github.com/arxanas/git-branchless/actions/workflows/windows.yml/badge.svg" /></a></td>
      <td align="center"><a href="https://github.com/arxanas/git-branchless/actions/workflows/macos.yml"><img alt="macOS build status" src="https://github.com/arxanas/git-branchless/actions/workflows/macos.yml/badge.svg" /></a></td>
      <td align="center"><a alt="Nix on Linus build status" href="https://github.com/arxanas/git-branchless/actions/workflows/nix-linux.yml"><img src="https://github.com/arxanas/git-branchless/actions/workflows/nix-linux.yml/badge.svg" /></a></td>
    </tr>
    <tr>
      <td align="center"><a alt="Package version" href="https://crates.io/crates/git-branchless"><img src="https://img.shields.io/crates/v/git-branchless" /></a></td>
      <td align="center"><a href="https://github.com/arxanas/git-branchless/discussions"><img alt="Github Discussions" src="https://img.shields.io/github/discussions/arxanas/git-branchless" /></a></td>
      <td align="center"><a href="https://discord.gg/caYQBJ82A4"><img alt="Discord" src="https://img.shields.io/discord/915309546984050709" /></a></td>
      <td align="center"><a href="https://gitpod.io/#https://github.com/arxanas/git-branchless/"><img height="20" src="https://gitpod.io/button/open-in-gitpod.svg" /></a></td>
    </tr>
  </tbody>
</table>

<hr />

<p align="center">
<a href="#installation">▼ Jump to installation ▼</a><br />
<a href="#table-of-contents">▼ Jump to table of contents ▼</a>
</p>

## About

`git-branchless` is a suite of tools which enhances Git in several ways:

It **makes Git easier to use**, both for novices and for power users. Examples:

  - [`git undo`](https://github.com/arxanas/git-branchless/wiki/Command:-git-undo): a general-purpose undo command. See the blog post <a href="https://blog.waleedkhan.name/git-undo/"><i>git undo: We can do better</i></a>.
  - [The smartlog](https://github.com/arxanas/git-branchless/wiki/Command:-git-smartlog): a convenient visualization tool.
  - [`git restack`](https://github.com/arxanas/git-branchless/wiki/Command:-git-restack): to repair broken commit graphs.
  - [Speculative merges](https://github.com/arxanas/git-branchless/wiki/Concepts#speculative-merges): to avoid being caught off-guard by merge conflicts.

It **adds more flexibility** for power users. Examples:

  - [Patch-stack workflows](https://jg.gg/2018/09/29/stacked-diffs-versus-pull-requests/): strong support for "patch-stack" workflows as used by the Linux and Git projects, as well as at many large tech companies. (This is how Git was "meant" to be used.)
  - [Prototyping and experimenting workflows](https://github.com/arxanas/git-branchless/wiki/Workflow:-divergent-development): strong support for prototyping and experimental work via "divergent" development.
  - [`git sync`](https://github.com/arxanas/git-branchless/wiki/Command:-git-sync): to rebase all local commit stacks and branches without having to check them out first.
  - [`git move`](https://github.com/arxanas/git-branchless/wiki/Command:-git-move): The ability to move subtrees rather than "sticks" while cleaning up old branches, not touching the working copy, etc.
  - [Anonymous branching](https://github.com/arxanas/git-branchless/wiki/Concepts#anonymous-branching): reduces the overhead of branching for experimental work.
  - In-memory operations: to modify the commit graph without having to check out the commits in question.
  - [`git next/prev`](https://github.com/arxanas/git-branchless/wiki/Command:-git-next,-git-prev): to quickly jump between commits and branches in a commit stack.
  - [`git co -i/--interactive`](https://github.com/arxanas/git-branchless/wiki/Command:-git-co): to interactively select a commit to check out.

It **provides faster operations** for large repositories and monorepos, particularly at large tech companies. Examples:
  - See the blog post <a href="https://blog.waleedkhan.name/in-memory-rebases/"><i>Lightning-fast rebases with git-move</i></a>.
  - Performance tested: benchmarked on [torvalds/linux](https://github.com/torvalds/linux) (1M+ commits) and [mozilla/gecko-dev](https://github.com/mozilla/gecko-dev) (700k+ commits).
  - Operates in-memory: avoids touching the working copy by default (which can slow down `git status` or invalidate build artifacts).
  - [Sparse indexes](https://github.blog/2021-11-10-make-your-monorepo-feel-small-with-gits-sparse-index/): uses a custom implementation of sparse indexes for fast commit and merge operations.
  - [Segmented changelog DAG](https://github.com/quark-zju/gitrevset/issues/1): for efficient queries on the commit graph, such as merge-base calculation in O(log n) instead of O(n).
  - Ahead-of-time compiled: written in an ahead-of-time compiled language with good runtime performance (Rust).
  - Multithreading: distributes work across multiple CPU cores where appropriate.
  - To my knowledge, `git-branchless` provides the *fastest* implementation of rebase among Git tools and UIs, for the above reasons.

See also the [User guide](https://github.com/arxanas/git-branchless/wiki) and [Design goals](https://github.com/arxanas/git-branchless/wiki/Design-goals).

## Table of contents

- [About](#about)
- [Demos](#demos)
  - [Repair](#repair)
  - [Visualize](#visualize)
  - [Manipulate](#manipulate)
- [Installation](#installation)
- [Status](#status)
- [Related tools](https://github.com/arxanas/git-branchless/wiki/Related-tools)
- [Contributing](#contributing)

## Demos
### Repair

Undo almost anything:

- Commits.
- Amended commits.
- Merges and rebases (e.g. if you resolved a conflict wrongly).
- Checkouts.
- Branch creations, updates, and deletions.

<p align="center">
<a href="https://asciinema.org/a/2hRDqRZKyppzmDL3Dz8zRleNd" target="_blank"><img src="https://asciinema.org/a/2hRDqRZKyppzmDL3Dz8zRleNd.svg" /></a>
</p>

<details>
<summary>Why not <code>git reflog</code>?</summary>

`git reflog` is a tool to view the previous position of a single reference (like `HEAD`), which can be used to undo operations. But since it only tracks the position of a single reference, complicated operations like rebases can be tedious to reverse-engineer. `git undo` operates at a higher level of abstraction: the entire state of your repository.

`git reflog` also fundamentally can't be used to undo some rare operations, such as certain branch creations, updates, and deletions. [See the architecture document](https://github.com/arxanas/git-branchless/wiki/Architecture#comparison-with-the-reflog) for more details.

</details>

<details>
<summary>What doesn't <code>git undo</code> handle?</summary>

`git undo` relies on features in recent versions of Git to work properly. See the [compatibility chart](https://github.com/arxanas/git-branchless/wiki/Installation#compatibility).

Currently, `git undo` can't undo the following. You can find the design document to handle some of these cases in [issue #10](https://github.com/arxanas/git-branchless/issues/10).

- "Uncommitting" a commit by undoing the commit and restoring its changes to the working copy.
  - In stock Git, this can be accomplished with `git reset HEAD^`.
  - This scenario would be better implemented with a custom `git uncommit` command instead. See [issue #3](https://github.com/arxanas/git-branchless/issues/3).
- Undoing the staging or unstaging of files. This is tracked by issue #10 above.
- Undoing back into the _middle_ of a conflict, such that `git status` shows a message like `path/to/file (both modified)`, so that you can resolve that specific conflict differently. This is tracked by issue #10 above.

Fundamentally, `git undo` is not intended to handle changes to untracked files.

</details>

<details>
<summary>Comparison to other Git undo tools</summary>

- [`gitjk`](https://github.com/mapmeld/gitjk): Requires a shell alias. Only undoes most recent command. Only handles some Git operations (e.g. doesn't handle rebases).
- [`git-extras/git-undo`](https://github.com/tj/git-extras/blob/master/man/git-undo.md): Only undoes commits at current `HEAD`.
- [`git-annex undo`](https://git-annex.branchable.com/git-annex-undo/): Only undoes the most recent change to a given file or directory.
- [`thefuck`](https://github.com/nvbn/thefuck): Only undoes historical shell commands. Only handles some Git operations (e.g. doesn't handle rebases).

</details>

### Visualize

Visualize your commit history with the smartlog (`git sl`):

<p align="center">
<img src="media/git-sl.png" /></a>
</p>

<details>
<summary>Why not <code>git log --graph</code>?</summary>

`git log --graph` only shows commits which have branches attached with them. If you prefer to work without branches, then `git log --graph` won't work for you.

To support users who rewrite their commit graph extensively, `git sl` also points out commits which have been abandoned and need to be repaired (descendants of commits marked with `rewritten as abcd1234`). They can be automatically fixed up with `git restack`, or manually handled.

</details>

### Manipulate

Edit your commit graph without fear:

<p align="center">
<a href="https://asciinema.org/a/3UVPMf0IpJaGdP6Kd6Zum4cq8" target="_blank"><img src="https://asciinema.org/a/3UVPMf0IpJaGdP6Kd6Zum4cq8.svg" /></a>
</p>

<details>
<summary>Why not <code>git rebase --interactive</code>?</summary>

Interactive rebasing with `git rebase --interactive` is fully supported, but it has a couple of shortcomings:

- `git rebase --interactive` can only repair linear series of commits, not trees. If you modify a commit with multiple children, then you have to be sure to rebase all of the other children commits appropriately.
- You have to commit to a plan of action before starting the rebase. For some use-cases, it can be easier to operate on individual commits at a time, rather than an entire series of commits all at once.

When you use `git rebase --interactive` with `git-branchless`, you will be prompted to repair your commit graph if you abandon any commits.

</details>

## Installation

See https://github.com/arxanas/git-branchless/wiki/Installation.

Short version: run `cargo install --locked git-branchless`, then run `git branchless init` in your repository.

## Status

`git-branchless` is currently in **alpha**. Be prepared for breaking changes, as some of the workflows and architecture may change in the future. It's believed that there are no major bugs, but it has not yet been comprehensively battle-tested. You can see the known issues in the [issue tracker](https://github.com/arxanas/git-branchless/issues/1).

`git-branchless` follows [semantic versioning](https://semver.org/). New 0.x.y versions, and new major versions after reaching 1.0.0, may change the on-disk format in a backward-incompatible way.

To be notified about new versions, select Watch » Custom » Releases in Github's notifications menu at the top of the page. Or use [GitPunch](https://gitpunch.com/) to deliver notifications by email.

## Related tools

There's a lot of promising tooling developing in this space. See [Related tools](https://github.com/arxanas/git-branchless/wiki/Related-tools) for more information.

## Contributing

Thanks for your interest in contributing! If you'd like, I'm happy to set up a call to [help you onboard](https://github.com/arxanas/git-branchless/wiki/Onboarding).

For code contributions, check out the [Runbook](https://github.com/arxanas/git-branchless/wiki/Runbook) to understand how to set up a development workflow, and the [Coding guidelines](https://github.com/arxanas/git-branchless/wiki/Coding). You may also want to read the [Architecture](https://github.com/arxanas/git-branchless/wiki/Architecture) documentation.

For contributing documentation, see the [Wiki style guide](https://github.com/arxanas/git-branchless/wiki/Wiki-style-guide).

Contributors should abide by the [Code of Conduct](https://github.com/arxanas/git-branchless/blob/master/CODE_OF_CONDUCT.md).
