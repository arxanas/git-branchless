//! Tools for editing the commit graph.

mod evolve;
mod execute;
pub mod hooks;
mod plan;

pub use evolve::{find_abandoned_children, find_rewrite_target};
pub use execute::{
    execute_rebase_plan, move_branches, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    MergeConflictInfo,
};
pub use plan::{BuildRebasePlanOptions, RebasePlanBuilder};
