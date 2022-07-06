//! Core functionality for git-branchless.

#![warn(missing_docs)]
#![warn(clippy::all, clippy::as_conversions, clippy::clone_on_ref_ptr)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

pub mod core;
pub mod git;
pub mod testing;
pub mod util;
