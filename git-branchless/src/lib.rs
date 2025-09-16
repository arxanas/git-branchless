//! Branchless workflow for Git.
//!
//! # Why?
//!
//! Most Git workflows involve heavy use of branches to track commit work that is
//! underway. However, branches require that you "name" every commit you're
//! interested in tracking. If you spend a lot of time doing any of the following:
//!
//!   * Switching between work tasks.
//!   * Separating minor cleanups/refactorings into their own commits, for ease of
//!     reviewability.
//!   * Performing speculative work which may not be ultimately committed.
//!   * Working on top of work that you or a collaborator produced, which is not
//!     yet checked in.
//!   * Losing track of `git stash`es you made previously.
//!
//! Then the branchless workflow may be for you instead.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_conditions)]

pub mod commands;
