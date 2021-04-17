# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- Fixed: Visible commits in the smartlog sometimes showed the reason that they were hidden, even though they were visible.
- Fixed: The working copy was sometimes left dirty after a `git undo`, even if it was clean beforehand.

## [0.3.1] - 2021-04-15

- Added: Hidden commits which appear in the smartlog now show the reason why they're hidden.
- Fixed: Historical commits displayed in `git undo` were sometimes rendered incorrectly, indicating that they were hidden/visible inappropriately. They now display the true historical visibility.

## [0.3.0] - 2021-04-08

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
