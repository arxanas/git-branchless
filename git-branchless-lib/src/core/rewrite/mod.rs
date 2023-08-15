//! Tools for editing the commit graph.

mod evolve;
mod execute;
mod plan;
pub mod rewrite_hooks;

use std::sync::Mutex;

pub use evolve::{find_abandoned_children, find_rewrite_target};
pub use execute::{
    execute_rebase_plan, move_branches, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    FailedMergeInfo, MergeConflictRemediation,
};
pub use plan::{
    BuildRebasePlanError, BuildRebasePlanOptions, OidOrLabel, RebaseCommand, RebasePlan,
    RebasePlanBuilder, RebasePlanPermissions,
};
use tracing::instrument;

use crate::core::task::{Resource, ResourcePool};
use crate::git::Repo;

/// A thread-safe [`Repo`] resource pool.
#[derive(Debug)]
pub struct RepoResource {
    repo: Mutex<Repo>,
}

impl RepoResource {
    /// Make a copy of the provided [`Repo`] and use that to populate the
    /// [`ResourcePool`].
    #[instrument]
    pub fn new_pool(repo: &Repo) -> eyre::Result<ResourcePool<Self>> {
        let repo = Mutex::new(repo.try_clone()?);
        let resource = Self { repo };
        Ok(ResourcePool::new(resource))
    }
}

impl Resource for RepoResource {
    type Output = Repo;

    type Error = eyre::Error;

    fn try_create(&self) -> Result<Self::Output, Self::Error> {
        let repo = self
            .repo
            .lock()
            .map_err(|_| eyre::eyre!("Poisoned mutex for RepoResource"))?;
        let repo = repo.try_clone()?;
        Ok(repo)
    }
}

/// Type synonym for [`ResourcePool<RepoResource>`].
pub type RepoPool = ResourcePool<RepoResource>;

/// Testing helpers.
pub mod testing {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use chashmap::CHashMap;

    use crate::core::dag::Dag;
    use crate::core::rewrite::{BuildRebasePlanOptions, RebasePlanPermissions};
    use crate::git::NonZeroOid;

    use super::RebasePlanBuilder;

    /// Create a `RebasePlanPermissions` that can rewrite any commit, for testing.
    pub fn omnipotent_rebase_plan_permissions(
        dag: &Dag,
        build_options: BuildRebasePlanOptions,
    ) -> eyre::Result<RebasePlanPermissions> {
        Ok(RebasePlanPermissions {
            build_options,
            allowed_commits: dag.query_all()?,
        })
    }

    /// Get the internal touched paths cache for a `RebasePlanBuilder`.
    pub fn get_builder_touched_paths_cache<'a>(
        builder: &'a RebasePlanBuilder,
    ) -> &'a CHashMap<NonZeroOid, HashSet<PathBuf>> {
        &builder.touched_paths_cache
    }
}
