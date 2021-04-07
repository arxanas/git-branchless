# Branchless workflow for Git

[![CI](https://github.com/arxanas/git-branchless/workflows/CI/badge.svg)](https://github.com/arxanas/git-branchless/actions?query=workflow%3ACI+branch%3Amaster)
[![crates.io](http://meritbadge.herokuapp.com/git-branchless)](https://crates.io/crates/git-branchless)

## Demo

[See the demo at asciinema](https://asciinema.org/a/ZHdMDW9997wzctW1T7QsUFe9G):

<p align="center">
<a href="https://asciinema.org/a/ZHdMDW9997wzctW1T7QsUFe9G" target="_blank"><img src="https://asciinema.org/a/ZHdMDW9997wzctW1T7QsUFe9G.svg" /></a>
</p>

## Why?

Most Git workflows involve heavy use of branches to track commit work that is underway. However, branches require that you "name" every commit you're interested in tracking. If you spend a lot of time doing any of the following:

  * Switching between work tasks.
  * Separating minor cleanups/refactorings into their own commits, for ease of
    reviewability.
  * Extensively rewriting local history before submitting code for review.
  * Performing speculative work which may not be ultimately committed.
  * Working on top of work that you or a collaborator produced, which is not
    yet checked in.
  * Losing track of `git stash`es you made previously.

Then the branchless workflow may be for you instead. 

The branchless workflow is designed for use at monorepo-scale, where the repository has a single main branch that all commits are applied to. It's based off the Mercurial workflows at large companies such as Google and Facebook. You can use it for smaller repositories as well, as long as you have a single main branch.

The branchless workflow is perfectly compatible with local branches if you choose to use them — they're just not necessary anymore.

## Installation

See https://github.com/arxanas/git-branchless/wiki/Installation.

Short version: run `cargo install git-branchless`, then run `git branchless init` in your repository.

## Status

`git-branchless` is currently in **beta**. It's believed that there are no major bugs, but it has not yet been comprehensively battle-tested. You can see the known issues in the [issue tracker](https://github.com/arxanas/git-branchless/issues/1).

`git-branchless` follows [semantic versioning](https://semver.org/). New 0.x.y versions, and new major versions after reaching 1.0.0, may change the on-disk format in a backward-incompatible way.

To be notified about new versions, select Watch » Custom » Releases in Github's notifications menu at the top of the page. Or use [GitPunch](https://gitpunch.com/) to deliver notifications by email.
