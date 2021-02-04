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
//!
//! # Branchless workflow and concepts
//!
//! The branchless workflow does away with needing to explicitly name commits
//! with branches (although you are free to do so if you like). Rather than use
//! branches to see your current work items, you simply make commits as you go.
//!
//! The branchless extensions infer which commits you're working on, and display
//! them to you with the `git smartlog` (or `git sl`) command.
//!
//! A commit is in one of three states:
//!
//!   * **Main**: A commit which has been checked into the main branch. No longer
//!     mutable. Visible to you in the branchless workflow.
//!   * **Visible**: A commit which you are working on currently. Visible to you in
//!     the branchless workflow.
//!   * **Hidden**: A commit which has been discarded or replaced. In particular,
//!     old versions of rebased commits are considered hidden. You can also
//!     manually hide commits that you no longer need. Not visible to you in the
//!     branchless workflow.

#![warn(clippy::all, missing_docs)]
#![allow(clippy::clippy::too_many_arguments)]

use log::warn;
use pyo3::prelude::*;
use python::map_err_to_py_err;
use simple_logger::SimpleLogger;

pub mod config;
pub mod eventlog;
pub mod gc;
pub mod graph;
pub mod init;
pub mod mergebase;
pub mod python;
pub mod restack;
pub mod util;

#[pymodule]
fn rust(_py: Python<'_>, module: &PyModule) -> PyResult<()> {
    map_err_to_py_err(SimpleLogger::new().init(), "Could not initialize logging")?;

    eventlog::register_python_symbols(&module)?;
    gc::register_python_symbols(&module)?;
    graph::register_python_symbols(&module)?;
    init::register_python_symbols(&module)?;
    mergebase::register_python_symbols(&module)?;
    restack::register_python_symbols(&module)?;

    Ok(())
}
