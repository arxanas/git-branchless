# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- next-header -->

## [Unreleased] - ReleaseDate

### Changed

- BREAKING (#1593): The minimum supported Rust version (MSRV) is now 1.82.
- `scm-record` upgraded to [v0.5.0](https://github.com/arxanas/scm-record/releases/tag/v0.5.0).
- (#1463): `git switch` now accepts a revset whose sole head will be checked out

## [v0.10.0] - 2024-10-10

### Added

- (#1355): `git submit` now supports the `--jobs` argument for parallelism.

### Changed

- `git2` upgraded to [v0.19.0](https://github.com/rust-lang/git2-rs/releases/tag/git2-0.19.0).
- `libgit2` upgraded to [v1.8.1](https://github.com/libgit2/libgit2/releases/tag/v1.8.1).
- `scm-record` upgraded to [v0.4.0](https://github.com/arxanas/scm-record/releases/tag/v0.4.0).

### Fixed

- (#1322): Fixed the processing of symbolic refs in reference-transaction lines (since Git v2.46+).
- (#1353): `git submit` with the Phabricator forge now ignores untracked working copy changes.
- (#1363): git-branchless now supports index version 4 (via `libgit2` upgrade).
- (#1393): Branches with multivars in their configuration can now be deleted (via `libgit2` upgrade).

## [v0.9.0] - 2024-05-27

### Added

- (#1129) Added a `--dry-run` option to `git submit` to report what would be submitted without actually doing so.
- (#1130) Added `merges()` revset function.
- (#1130) The `branches()` revset function now accepts an optional text pattern argument to limit which branches are matched.
- (#1150) The `git record` command now accepts `-s`/`--stash` to return to the previous commit immediately after committing.
- (#1167) The commit message for a new commit can now be written/edited during `git record --interactive`.
- (#1169) `git record` now accepts multiple `--message` arguments.
- (#1184) An initial Github forge was implemented as `git submit --forge github`, but it's [too buggy for general use](https://github.com/arxanas/git-branchless/discussions/1259).
- (#1241) `git smartlog` now accepts `--exact` to skip rendering `HEAD` and the main branch.
- (#1244) `git submit` now accepts multiple arguments/revsets.

### Changed

- `scm-record` upgraded to [v0.3.0](https://github.com/arxanas/scm-record/releases/tag/v0.3.0).
- BREAKING (#1128) Arguments/revsets passed to `git sync` are now resolved to their respective stacks.
  - This allows `git sync my-branch` to work as expected, instead of needing to use `git sync 'stack(my-branch)'`. The behavior of `git sync` when called without arguments is not affected by this change. If you rely on the previous behavior, please use `git move -x <commit(s)/revset> -d 'main()'` instead.
- BREAKING (#1152) Previously, `git hide` would not delete branches pointing to the hidden commits unless `-D`/`--delete-branches` was passed. Now, deleting branches is the default behavior. Pass `--no-delete-branches` to restore the old behavior.
- BREAKING (#1292): The minimum supported Rust version (MSRV) is now 1.82.
- (#1204) The default instructions for `git reword` are now wrapped to 72 characters.
- (#1230) The icon for omitted commits in the smartlog was changed from `⊘` to `◌`.

### Fixed

- (#1071) The Apache and MIT licenses are now distributed with each constituent crate, not just the top-level `git-branchless` crate.
- (#1072) The current branch is no longer detached during `git amend` when the current commit has descendants.
- (#1073) Merge commits can now be amended with `git amend`.
- (#1095) The event log is now shared between all worktrees. Before, commits that were made in one worktree wouldn't be visible in other worktrees, etc.
- (#1095) `git submit` now runs in worktree that you invoked it in.
- (#1095) `git submit --forge phabricator` no longer records spurious commits when `arc diff`ing.
- (#1127) Improved support for files with spaces in their name.
- (#1267) The correct "path" variable is now used on Windows, which fixes some cases of `git-branchless` failing on native Windows.

## [v0.8.0] - 2023-08-27

### Added

- (#545) EXPERIMENTAL: Added a `--fixup` option to `git move` to squash moved commits into the destination
- (#830) Added `git branchless install-man-pages` command. This may be useful for package maintainers or those who install git-branchless from source.
- (#840) git-branchless supports Phabricator as a backend forge for `git submit`.
- (#845) Added the `branchless.smartlog.defaultRevset` configuration variable.
- (#910) Added support for repositories managed by the `repo` tool.
- (#1009) Added `git branchless difftool` subcommand.

### Changed

- BREAKING (#841): git-branchless is now dual-licensed as MIT/Apache 2.0.
- BREAKING: (#1024) Rust v1.67 or later is required to build.
- (#825) `git submit` only fetches the necessary branches, rather than all branches, before pushing.
- (#826) Switch to `scm-record` crate to provide the partial commit interface.
- (#914) The default revset for `git restack` is now `draft()`.

### Fixed

- (#866) New commits created during `git rebase` are no longer kept if the rebase is aborted.
- (#866) `git test` no longer clobbers working copy changes.
- (#876) After `git amend`, detached branches previously pointing to the amended commit are now updated.
- (#915) Fixed certain situations of rebasing merge commits when all parents have also been rebased.
- (#920) Running `git sync` when the main branch is checked out no longer leaves a dirty index.
- (#938) Somehow fixed mysterious hang.

## [0.7.1] - 2023-03-13

### Added

- (#830) Added `git branchless install-man-pages` command to generate and install man-pages. This is primarily intended for those packaging git-branchless for package management systems.

### Fixed

- (#844) Fixed build for NetBSD.

## [0.7.0] - 2023-03-01

### Added

- (#646) `git amend` now supports a `--reparent` option to adjust the contents of a commit while keeping the children commits exactly the same.
- (#686) `git branchless init` will now populate the internal commit graph structure, rather than waiting for the first invocation of a `git branchless` subcommand.
- (#725) Added `siblings()` revset function to determine the siblings of a given commit.
- (#725) Added an `-I`/`--insert` option to `git record` to insert the new commit before all of the current commit's children.
- (#763) Added a `-d`/`--detach` option to `git branchless switch` to switch to a commit without checking out any associated branches (as might happen due to the `branchless.navigation.autoSwitchBranches` configuration variable), or to switch to a branch without checking it out.
- (#766) Added a `git test fix` subcommand to apply formatters/linters/etc. to a set of commits.
- (#777) Added the `--search` option to `git test run` to search a set of commits using linear or binary search for the first commit(s) which cause the tests to fail.
- (#777) `git test run` now aborts the overall test run when a test returns exit code `127`.
- (#777) `git test run` now sets the `BRANCHLESS_TEST_COMMIT` and `BRANCHLESS_TEST_COMMAND` environment variables when running the test command.
- (#785) Added `tests.passed()`, `tests.failed()`, and `tests.fixable()` revset functions, whose results are populated by `git test`.
- (#790) Added the `--reverse` option to `git smartlog`.

### Changed

- (#730) BREAKING: The default revset for `git reword` is now `stack() | @` instead of `@`, to simultaneously reword all commits in the current stack.
- (#763) BREAKING: `git branchless switch` no longer implicitly opens the interactive commit selector when no target is provided. You must explicitly pass `-i`/`--interactive` to do so.
- (#801) BREAKING: The parameter to `git record` has been renamed from `-b`/`--branch` to `-c`/`--create`.
- (#685) `git submit` now colorizes the names of the affected branches.
- (#763) Running `git branchless switch` with no arguments will switch to the branch associate with the current commit, if there is exactly one such branch and the `branchless.navigation.autoSwitchBranches` configuration variable is set to `true`.
- (#791) The name for the temporary file created by `git reword` is now of the form `COMMIT_EDITMSG-*.txt`, which your editor can use to detect it as a Git commit message file.
- (#811) The styling for `git test` progress and output has been changed.

### Fixed

- (#646) Adjusted messaging during merge conflicts with `git move --in-memory` to be more accurate.
- (#647) `git test run` now recovers from previously-failed tests instead of failing indefinitely in future runs.
- (#670) `git submit` no longer force-pushes the current branch in the circumstance that all branches are currently up-to-date.
- (#688) `git amend` now respects the `--merge` option.
- (#722) `git reword` now supports invoking editors with spaces in their names.
- (#724) `git branchless init` now installs a `post-applypatch` hook, for users of `git am`.
- (#742) `git branchless init` now respects a leading `~` in the `core.hooksPath` configuration variable.
- (#743) `git sync`, etc. now correctly clean up remote branch information for branches which have been merged upstream. Previously, the remote branch information would be left behind, which would cause future branches with the same names to incorrectly become associated with the old remote branch.
- (#764) `git sync` will no longer attempt to resolve merge conflicts unless you pass `--merge`. Previously, this could happen when attempting to sync merge commits.

## [0.6.0] - 2022-11-14

### Added

- (#576) EXPERIMENTAL: Created `git test` command to run a command on each commit in your stack.
- (#589, #593) `git sync --pull` rebases the local main branch on top of the remote main branch.
- (#619) `git smartlog` now renders revsets with non-contiguous commits.
- (#638) `git smartlog` now deduplicates merge commits which appear in the smartlog.

### Changed

- (#526) BREAKING: When checking out a commit with a single branch attached to it, that branch will also be checked out.
- (#589) BREAKING: Remote main branches are no longer supported. To upgrade you'll need to create a local main branch which tracks the remote main branch. You can use `git sync` to keep it up-to-date.
- (#571) Added `current(<revset>)` revset function to determine the latest versions of a set of rewritten commits.
- (#582) Added `main()`, `public()` revset functions.
- (#593, #608) `git sync` produces more descriptive output.

### Fixed

- (#588) `git branchless init` now works when invoked in a worktree.
- (#589) `git reword --force-rewrite` no longer moves remote-tracking branches.
- (#589) `git smartlog` no longer uses 100% CPU in certain cases with remote main branches (because remote main branches are no longer supported).

## [0.5.0] - 2022-09-28

### Added

- (#508) Added `exactly(<revset>, n)` revset function to allow assertions on the number of commits within a set.
- (#509) Users can now define custom [revset aliases](https://github.com/arxanas/git-branchless/wiki/Reference:-Revsets#Aliases).
- (#533) `git reword` can now reword merge commits.
- (#534) `git record` now accepts a `--detach` option to avoid moving the current branch.
- (#538) `git reword` now accepts a `--fixup` option to convert regular commits into `fixup!` commits (for use with `git rebase --autosquash`)
- (#541) EXPERIMENTAL: Created `git submit` command. Discuss at https://github.com/arxanas/git-branchless/discussions/564.

### Changed

- (#543) Commands will produce a better error message for certain unsupported sparse checkout configurations.

### Fixed

- (#507) The `messages()` revset function now ignores trailing newlines in commit messages.
- (#512) Fixed so that the setting for `--color` is now respected.
- (#512) Fixed so that you can pass `--color` anywhere in the command-line, not just before the subcommand.
- (#513) Fixed some false positives for hints to run `git restack` when printing the smartlog.
- (#518) Fixed the suggested flag in the error message when attempting to rewrite public commits.
- (#539) Fixed parse errors for certain single-quoted string literals in revset expressions.
- (#559) Fixed precedence for parentheses in revset expressions.
- (#569) Fixed the reversed output order of the results of `git query`.

## [0.4.0] - 2022-08-09

### Added

- Numerous updates to `git branchless move`:
  - (#400) Added `--insert` option to to insert the moved subtree or commits into the commit tree between the destination commit and its children, if any.
  - (#450) Added `--exact` option to move a specific set of commits elsewhere in the commit tree. They will be removed from their current location and any unmoved children (if any) will be moved to their closest unmoved ancestor. The moved commits will maintain their overall ancestry structure.
  - (#447) `--source`, `--base` and `--exact` options can all be provided multiple times to move multiple subtress, ranges and/or commits to the destination.
- Added `--yes` option to `git undo` to skip confirmation.
- (#366) Added bulk edit mode to `git reword` to allow updating multiple, individual commit messages at once.
- (#398) Added support for [revset expressions](https://github.com/arxanas/git-branchless/wiki/Reference:-Revsets) in most commands.
  - Created the `git query` command to dump the results of revset queries.
- (#399) Added `--delete-branches` option to `git hide`.
- (#435) Created the `git branchless repair` command, which is occasionally useful for cleaning up garbage-collected commits from the smartlog.
- (#382) EXPERIMENTAL: Working copy snapshots are created before potentially-destructive operations in order to improve the capabilities of `git undo`.
  - Working copy snapshots are taken by default. Disable by setting `branchless.undo.createSnapshots` to `false`.
- (#391) EXPERIMENTAL: created the `git record` command, which opens an interactive change selector for committing.

### Changed

- BREAKING: (#422) Rust v1.61 or later is required to build.
- BREAKING: (#472) `git smartlog` no longer supports the `--only-branches` option. Instead, use `git smartlog 'branches()'`.
- BREAKING: (#479) `git move` will abort when trying to move public commits; you now have to pass `--force`.
- (#397) When editing only a single commit message with `git reword`, the marker line is omitted.

### Fixed

- (#342) Commands which automatically show the smartlog after them no longer show it in gray with ASCII glyphs. This reverts the behavior to that of before v0.3.11.
- (#358) Adjusted the misleading suggestion that was printed when running a `git amend` invocation which produced merge conflicts.
- (#359) Fixed the `smartlog` output when running `git undo`. Previously, it showed the smartlog as if you were still at the `HEAD` location before the `git undo`.
- (#387) `git reword` now reads from `$GIT_EDITOR`, etc. via the `git var` command.
- (#418) When running `git smartlog` or similar, any commits which would show up are now kept live for garbage-collection purposes, even if `git-branchless` didn't observe them being committed.

## [0.3.12] - 2022-04-08

No changes.

## [0.3.11] - 2022-04-06

### Added

- Added `--discard` flag to `git reword` to start with a clean commit message.

### Fixed

- (#326) `git restack` now restacks certain branch commits which it missed before.

## [0.3.10] - 2022-03-27

NOTE: when installing this version with `--locked`, you may see a warning like this:

```
warning: package `crossbeam-channel v0.5.2` in Cargo.lock is yanked in registry `crates-io`, consider running without --locked
```

This is safe to ignore. We are waiting for an upstream dependency to be updated to resolve this warning. This is tracked in https://github.com/arxanas/git-branchless/issues/317.

### Added

- EXPERIMENTAL: created `git sync` command, which moves all commit stacks onto the main branch (if possible).
- EXPERIMENTAL: created `git reword` command, which can rewrite commit messages anywhere in the commit tree.
- EXPERIMENTAL: created `git branchless bug-report` command, which produces information that can be attached to an issue on Github.
- The `--only-branches` option can be passed to `git smartlog` to only show commits which are on branches.
- The `git move` command, and other commands which can move commits, now accepts the option `--no-deduplicate-commits` to skip commit deduplication.

### Changed

- (#286) The smartlog now displays an icon next to the currently-checked-out branch.
- (#289) Changed output wording for `git hide`/`git unhide`.
- BREAKING: `git undo` now undoes the most recent operation by default (after confirming). The interactive behavior is available with the `-i`/`--interactive` flag.

### Fixed

- (#267) Aliases like `git amend` are now installed only if the user does not already have aliases with the same name. Thanks to @rslabbert for implementing this.
- Improved performance up to 15x for `git restack` on large commit histories.
- (#280) Ambiguous commit hashes are no longer printed in output. (Additional characters of the hash will be appended as necessary.) Thanks to @yujong-lee for fixing this.

## [0.3.9] - 2022-02-08

### Fixed

- (#249) On-disk `git move` commands now ensure that they keep your current branch checked out after the operation.
- (#249) On-disk `git move` commands no longer mysteriously move your current branch to a different commit in the stack.

## [0.3.8] - 2021-12-04

### Added

- New `git branchless checkout` command, which enables you to interactively pick a commit to checkout from the commits tracked in the smartlog.
- `git next` accepts an `--interactive` flag which, if set, prompts which commit to advance to in ambiguous circumstances. This can be enabled by default with the `branchless.next.interactive` config setting.
- References created for garbage collection purposes no longer appear in `git log` by default.
- References created for garbage collection purposes are no longer generated unless you've run `git branchless init` in the repository.
- `git next` and `git prev` accept `-a`/`--all` to take you all the way to a head or root commit for your commit stack, respectively.
- `git next` and `git prev` accept `-b`/`--branch` to take you to the next or previous branch for your commit stack, respectively.
- New `git branchless amend` command that amends the current HEAD commit, and automatically performs a restack.
- `git next` and `git prev` accept `-m`/`--merge` to merge unstaged changes when checking out to the destination commit.
- `git next` and `git prev` accept `-f`/`--force` to discard unstaged changes when checking out to the destination commit.
- `git branchless init` warns if the configuration value `core.hooksPath` is set.

### Fixed

- (#151) `ORIG_HEAD` is populated correctly, which means that Git commands which write to `ORIG_HEAD` don't accidentally clobber unrelated branches.
- (#155) `git branchless init` now appends to your existing hooks, rather than silently doing nothing.
- (#172) When carrying out an on-disk rebase operation with `git move`, calling `git rebase --abort` will correctly reset the branch which you had checked out prior to the rebase.
- (#209) `git restack` no longer resurrects commits which were created before `git branchless init` was run.
- (#236) `git restack` no longer checks out back to an abandoned commit in some circumstances.

## [0.3.7] - 2021-10-22

### Added

- `git branchless init` takes a `--main-branch` option to specify the name of the main branch without interactive prompting.
- The `--color=[auto,always,never]` flag can be used to override the automatically detected value for terminal colors.
- The `CLICOLOR` and `NO_COLOR` environment variables are now respected.

### Changed

- BREAKING: If your local main branch has an upstream branch, then that upstream branch will be treated as the repository's main branch, and your local main will be treated as a branch like any other. This should make workflows which commit to the main branch more ergonomic.
- BREAKING: `git move` and `git restack` will no longer perform merge conflict resolution unless the `--merge` option was passed.
- `git branchless init` will use `init.defaultBranch` when detecting the name of the main branch, if one is not provided by `--main-branch`.
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
@NOCOMMIT: update comment/link
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
