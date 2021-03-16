# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## v0.2.0 - 2020-03-15

Ported to Rust. No new features.

* Performance for repeated calls to Git hooks is significantly improved. This can happen when rebasing large commit stacks.
* The `git undo` UI has been changed to use a Rust-specific TUI library (`cursive`).

## v0.1.0 - 2020-12-18

First beta release. Supports these commands:

* `git sl`/`git smartlog`.
* `git hide`/`git unhide`.
* `git prev`/`git next`.
* `git restack`.
* `git undo`.
