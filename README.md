# Branchless workflow for Git

## Demo

[See the demo at asciinema](https://asciinema.org/a/ZHdMDW9997wzctW1T7QsUFe9G).

## Why?

Most Git workflows involve heavy use of branches to track commit work that is underway. However, branches require that you "name" every commit you're interested in tracking. If you spend a lot of time doing any of the following:

  * Switching between work tasks.
  * Separating minor cleanups/refactorings into their own commits, for ease of
    reviewability.
  * Performing speculative work which may not be ultimately committed.
  * Working on top of work that you or a collaborator produced, which is not
    yet checked in.
  * Losing track of `git stash`es you made previously.

Then the branchless workflow may be for you instead. 

The branchless workflow is designed for use at monorepo-scale, where the repository has a single main branch that all commits are applied to. It's based off of [Facebook's hg smartlog extension](https://www.mercurial-scm.org/wiki/SmartlogExtension) and related tooling. You can use it for smaller repositories as well, as long as you have a single main branch.

The branchless workflow is perfectly compatible with local branches if you choose to use them — they're just not necessary anymore.

## Installation

Requires Python 3.6 or higher. To install:

```
$ pip3 install git+https://github.com/arxanas/git-branchless
```

Then enable it in your Git repository with `git branchless init`:

```
$ cd my/git/repo
$ git branchless init
Installing hook: post-commit
Installing hook: post-rewrite
Installing hook: post-checkout
Installing alias (non-global): git smartlog
Installing alias (non-global): git sl
Installing alias (non-global): git hide
Installing alias (non-global): git unhide
Installing alias (non-global): git prev
Installing alias (non-global): git next
Installing alias (non-global): git restack
Installing alias (non-global): git undo
$ git sl
⋮
◆ b2c57ae3 9m (master) docs: add README.md
```

To uninstall:

```
$ pip3 uninstall branchless
```

## Status

`git-branchless` is not yet suited for general use: commits in the smartlog are not explicitly referenced in Git's `refs` namespace, which means that an unfortunate garbage collection could remove them.
