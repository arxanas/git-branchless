//! Supporting library for
//! [git-branchless](https://github.com/arxanas/git-branchless).
//!
//! This is a UI component to interactively select changes to include in a
//! commit. It's meant to be embedded in source control tooling.
//!
//! You can think of this as an interactive replacement for `git add -p`, or a
//! reimplementation of `hg crecord`. Given a set of changes made by the user,
//! this component presents them to the user and lets them select which of those
//! changes should be staged for commit.

#![warn(missing_docs)]
#![warn(clippy::all, clippy::as_conversions, clippy::clone_on_ref_ptr)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

mod cursive_utils;
mod tristate;
mod types;
mod ui;
mod views;

pub use cursive_utils::testing;
pub use types::{FileState, RecordError, RecordState, Section, SectionChangedLine};
pub use ui::Recorder;
