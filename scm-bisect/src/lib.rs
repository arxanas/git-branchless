//! Reusable algorithms for identifying the first bad commit in a directed
//! acyclic graph (similar to `git-bisect`). The intention is to provide support
//! for various source control systems.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_conditions)]

pub mod basic;
pub mod search;

#[cfg(test)]
pub mod testing;
