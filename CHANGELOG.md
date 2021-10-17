# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- New `git branchless checkout` command, which enables you to interactively pick a commit to checkout from the commits tracked in the smartlog.

## [0.3.7] - 2021-10-22

### Added

- `git branchless init` takes a `--main-branch` option to specify the name of the main branch without interactive prompting.
- The `--color=[auto,always,never]` flag can be used to override the automatically detected value for terminal colors.
- The `CLICOLOR` and `NOCOLOR` environment variables are now respected.

### Changed

- BREAKING: If your local main branch has an upstream branch, then that upstream branch will be treated as the repository's main branch, and your local main will be treated as a branch like any other. This should make workflows which commit to the main branch more ergonomic.
- BREAKING: `git move` and `git restack` will no longer perform merge conflict resolution unless the `--merge` option was passed.
  `git branchless init` will use `init.defaultBranch` when detecting the name of the main branch, if one is not provided by `--main-branch`.
- (#144) When automatic garbage collection is run, the number of deleted references is displayed.

### Fixed

- On-disk rebases on systems with `/tmp` residing on a different filesystem should no longer fail.
- (#129) `git move` operations with `--dest` referring to a remote commit no longer panic.

## [0.3.6] - 2021-10-14

### Added

- The `-C` option can be used to set the working directory for `git-branchless` commands.
- The `--hidden` option can be passed to `git smartlog` to show commits which are not ordinarily visible.

### Changed

- Git configuration is written to a file under `.git/branchless`, instead of writing it directly to `.git/config` (which may clobber user settings).

### Fixed

- Output of subcommands is no longer overwritten by progress updates.
- Improved performance up to 100x for commit deduplication during `git move` when rebasing past certain large commits.
- Improved performance up to 10x for smartlog rendering.

## [0.3.5] - 2021-09-11

### Added

- Merge commits can be rebased by `git move --on-disk`. This uses the same system as `git rebase --rebase-merges`.

### Changed

- (#63) The UI for `git undo` has been changed in various ways. Thanks to @chapati23 for their feedback. You can leave your own feedback here: https://github.com/arxanas/git-branchless/discussions
- Merge-base calculation is now performed using [EdenSCM](https://github.com/facebookexperimental/eden)'s directed acyclic graph crate ([`esl01-dag`](https://crates.io/crates/esl01-dag)), which significantly improves performance on large repositories.
- Subprocess command output is now dimmed and printed above a progress meter, to make it easier to visually filter out important `git-branchless` status messages from unimportant `git` machinery output.
- `git move` tries to avoid issuing a superfluous `git checkout` operation if you're already at the target commit/branch.
- `git restack` uses in-memory rebases by default.

### Fixed

- `git restack` warns if a sub-command fails (e.g. if `git rebase` fails with merge conflicts that need to be resolved).
- (#57) `git undo` shows an informative link when dealing with empty events, rather than warning about a bug. Thanks to @waych for reporting.
- Flickering in `git undo`'s rendering has been reduced.
- Commits made via `git merge` are now recorded in the event log.
- Long progress messages are now truncated on narrow screens.
- In-memory rebases on large repositories are now up to 500x faster. See https://github.com/libgit2/libgit2/issues/6036.
- `git smartlog` no longer crashes after you've just run `git checkout --orphan <branch>`.
- In-memory diffs on large repositories (used for commit deduplication) are now up to 100x faster. See https://github.com/libgit2/libgit2/issues/6036.
- Invocations of `git-branchless` commands which called subprocesses and then exited quickly no longer fail to print the subprocess output.

## [0.3.4] - 2021-08-12

### Added

- `git move` now supports forcing an in-memory rebase with the `--in-memory` flag.
- The `reference-transaction` hook prints out which references were updated.
- `git restack` can now accept a list of commit hashes whose descendants should be restacked, rather than restacking every abandoned commit indiscriminately.
- `git move` will skip applying commits which have already been applied upstream, and delete their corresponding branches.
- Progress indicators are now displayed when `git-branchless` takes longer than 250ms to complete.

### Changed

- BREAKING: `git-branchless` is now licensed under the GPL-2.
- More of the Git hooks installed by `git-branchless` display the affected objects, rather than just the number of affected objects.
- `git move` with no `--source` or `--base` option now defaults to `--base HEAD` rather than `--source HEAD`.

### Fixed

- The output of `git` subcommands is streamed to stdout, rather than accumulated and dumped at the end.
- Commits rebased in-memory by `git move` are now marked as reachable by the Git garbage collector, so that they aren't collected prematurely.
- `git-branchless wrap` correctly relays the exit code of its subprocess.
- Some restack and move operations incorrectly created branches without the necessary `refs/heads/` prefix, which means they weren't considered local branches by Git.
- Some restack and move operations didn't relocate all commits and branches correctly, due to the experimental `git move` backend. The backend has been changed to use a constraint-solving approach rather than a greedy approach to fix this.
- `git move` preserves committer timestamps when `branchless.restack.preserveTimestamps` is set. The configuration key may change in the future.
- If your currently-checked-out commit was rewritten during a `git move` operation, it now checks out the new version of the commit, rather than leaving you on an old, hidden commit.
- If your current stack had another stack branching off of it, and `git move --base` was passed a commit from that other stack, it would fail with a cyclic dependency error. It now clips off the unique part of the branch and moves it.
- If an on-disk rebase would occur (such as the result of `git move` or `git restack`), but you have uncommitted changes in your working copy, the rebase is aborted and a warning is printed, rather than potentially clobbering your changes.

## [0.3.3] - 2021-06-27

### Added

- `git branchless init` will attempt to detect the correct main branch name to use for the repository. If not automatically detected, it will prompt for the branch name.
- `git branchless init --uninstall` will uninstall `git-branchless` from the repository.

### Fixed

- The version number in `git-branchless --help` was fixed at `0.2.0`. It now reflects the version of the package.
- `git branchless wrap` no longer fails to run if there is no Git repository in the current directory.
- User hooks which are invoked by `git-branchless` are now invoked in the correct working directory.

## [0.3.2] - 2021-06-23

### Added

- `git branchless init` now sets `advice.detachedHead false`, to reduce the incidence of scary messages.
- Aliasing `git` to `git-branchless wrap` improves which commands are grouped together for `git undo`, and possibly enables more features in the future.
- `git-branchless` builds on Windows (#13, #20).
- EXPERIMENTAL: Created `git move` command, which rebases entire subtrees at once. Not currently stable.

### Changed

- BREAKING: The configuration option `branchless.mainBranch` has been renamed to `branchless.core.mainBranch`. The old option will be supported indefinitely, but eventually removed.

### Fixed

- Visible commits in the smartlog sometimes showed the reason that they were hidden, even though they were visible.
- The working copy was sometimes left dirty after a `git undo`, even if it was clean beforehand.
- `git-branchless` now supports Git v2.31.
- `git restack` now doesn't infinite-loop on certain rebase conflict scenarios.
- `git smartlog` now doesn't crash for some cases of hidden merge commits.
- `git-branchless` bundles its own version of SQLite, so that the user doesn't need to install SQLite as a dependency themselves (#13).

## [0.3.1] - 2021-04-15

### Added

- Hidden commits which appear in the smartlog now show the reason why they're hidden.

### Fixed

- Historical commits displayed in `git undo` were sometimes rendered incorrectly, indicating that they were hidden/visible inappropriately. They now display the true historical visibility.

## [0.3.0] - 2021-04-08

### Changed

- BREAKING: Events are now grouped into transactions. This improves the UX around `git undo`, since it can undo groups of related events. This breaks the on-disk database format.

## [0.2.0] - 2020-03-15

Ported to Rust. No new features.

- Performance for repeated calls to Git hooks is significantly improved. This can happen when rebasing large commit stacks.
- The `git undo` UI has been changed to use a Rust-specific TUI library (`cursive`).

## [0.1.0] - 2020-12-18

First beta release. Supports these commands:

- `git sl`/`git smartlog`.
- `git hide`/`git unhide`.
- `git prev`/`git next`.
- `git restack`.
- `git undo`.
