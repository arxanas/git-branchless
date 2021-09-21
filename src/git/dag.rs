use std::collections::HashSet;
use std::convert::TryFrom;

use eden_dag::ops::DagPersistent;
use eden_dag::{DagAlgorithm, Set, Vertex};
use eyre::Context;
use itertools::Itertools;
use tracing::{instrument, trace, warn};

use crate::core::eventlog::EventReplayer;
use crate::git::{Commit, MaybeZeroOid, NonZeroOid, Repo};
use crate::tui::{Effects, OperationType};

impl From<NonZeroOid> for Vertex {
    fn from(oid: NonZeroOid) -> Self {
        Vertex::copy_from(oid.as_bytes())
    }
}

impl TryFrom<Vertex> for NonZeroOid {
    type Error = eyre::Error;

    fn try_from(value: Vertex) -> Result<Self, Self::Error> {
        let oid = git2::Oid::from_bytes(value.as_ref())?;
        let oid = MaybeZeroOid::from(oid);
        NonZeroOid::try_from(oid)
    }
}

/// Interface to access the directed acyclic graph (DAG) representing Git's
/// commit graph. Based on the Eden SCM DAG.
pub struct Dag {
    inner: eden_dag::Dag,
}

impl Dag {
    /// Initialize the DAG for the given repository.
    #[instrument]
    pub fn open(
        effects: &Effects,
        repo: &Repo,
        event_replayer: &EventReplayer,
    ) -> eyre::Result<Self> {
        // There's currently no way to view the DAG as it was before a
        // certain event, so just use the cursor for the present time.
        let event_cursor = event_replayer.make_default_cursor();
        let commit_oids = event_replayer.get_cursor_oids(event_cursor);
        let main_branch_oid = repo.get_main_branch_oid()?;

        let master_oids = {
            let mut result = HashSet::new();
            result.insert(main_branch_oid);
            result
        };
        let non_master_oids = {
            let mut result = commit_oids;
            result.remove(&main_branch_oid);
            result
        };

        let dag_dir = repo.get_dag_dir()?;
        let dag = {
            let mut dag = eden_dag::Dag::open(&dag_dir)
                .wrap_err_with(|| format!("Opening DAG directory at: {:?}", &dag_dir))?;
            Self::update_oids(effects, repo, &mut dag, &master_oids, &non_master_oids)?;
            dag
        };

        Ok(Self { inner: dag })
    }

    /// This function's code adapted from `GitDag`, licensed under GPL-2.
    fn update_oids(
        effects: &Effects,
        repo: &Repo,
        dag: &mut eden_dag::Dag,
        master_oids: &HashSet<NonZeroOid>,
        non_master_oids: &HashSet<NonZeroOid>,
    ) -> eyre::Result<()> {
        let (_effects, _progress) = effects.start_operation(OperationType::UpdateCommitGraph);

        let parent_func = |v: Vertex| -> eden_dag::Result<Vec<Vertex>> {
            use eden_dag::errors::BackendError;
            trace!(?v, "visiting Git commit");

            let oid = MaybeZeroOid::from_bytes(v.as_ref())
                .map_err(|_e| anyhow::anyhow!("Could not convert to Git oid: {:?}", &v))
                .map_err(BackendError::Other)?;
            let oid = match oid {
                MaybeZeroOid::NonZero(oid) => oid,
                MaybeZeroOid::Zero => return Ok(Vec::new()),
            };

            let commit = repo
                .find_commit(oid)
                .map_err(|_e| anyhow::anyhow!("Could not resolve to Git commit: {:?}", &v))
                .map_err(BackendError::Other)?;
            let commit = match commit {
                Some(commit) => commit,
                None => {
                    // This might be an OID that's been garbage collected, or
                    // just a non-commit object. Ignore it in either case.
                    return Ok(Vec::new());
                }
            };

            Ok(commit
                .get_parent_oids()
                .into_iter()
                .map(Vertex::from)
                .collect())
        };

        dag.add_heads_and_flush(
            parent_func,
            master_oids
                .iter()
                .copied()
                .map(Vertex::from)
                .collect_vec()
                .as_slice(),
            non_master_oids
                .iter()
                .copied()
                .map(Vertex::from)
                .collect_vec()
                .as_slice(),
        )?;
        Ok(())
    }

    /// Get one of the merge-base OIDs for the given pair of OIDs. If there are
    /// multiple possible merge-bases, one is arbitrarily returned.
    #[instrument]
    pub fn get_one_merge_base_oid(
        &self,
        effects: &Effects,
        repo: &Repo,
        lhs_oid: NonZeroOid,
        rhs_oid: NonZeroOid,
    ) -> eyre::Result<Option<NonZeroOid>> {
        let set = vec![Vertex::from(lhs_oid), Vertex::from(rhs_oid)];
        let set = self
            .inner
            .sort(&Set::from_static_names(set))
            .wrap_err("Sorting DAG vertex set")?;
        let vertex = self.inner.gca_one(set).wrap_err("Computing merge-base")?;
        match vertex {
            None => Ok(None),
            Some(vertex) => Ok(Some(vertex.to_hex().parse()?)),
        }
    }

    /// Get the range of OIDs from `parent_oid` to `child_oid`. Note that there
    /// may be more than one path; in that case, the OIDs are returned in a
    /// topologically-sorted order.
    #[instrument]
    pub fn get_range(
        &self,
        effects: &Effects,
        repo: &Repo,
        parent_oid: NonZeroOid,
        child_oid: NonZeroOid,
    ) -> eyre::Result<Vec<NonZeroOid>> {
        let roots = Set::from_static_names(vec![Vertex::from(parent_oid)]);
        let heads = Set::from_static_names(vec![Vertex::from(child_oid)]);
        let range = self.inner.range(roots, heads).wrap_err("Computing range")?;
        let range = self.inner.sort(&range).wrap_err("Sorting range")?;
        let oids = {
            let mut result = Vec::new();
            for vertex in range.iter()? {
                let vertex = vertex?;
                let oid = vertex.as_ref();
                let oid = MaybeZeroOid::from_bytes(oid)?;
                match oid {
                    MaybeZeroOid::Zero => {
                        // Do nothing.
                    }
                    MaybeZeroOid::NonZero(oid) => result.push(oid),
                }
            }
            result
        };
        Ok(oids)
    }

    /// Find a shortest path between the given commits.
    ///
    /// This is particularly important for multi-parent commits (i.e. merge commits).
    /// If we don't happen to traverse the correct parent, we may end up traversing a
    /// huge amount of commit history, with a significant performance hit.
    ///
    /// Args:
    /// * `repo`: The Git repository.
    /// * `commit_oid`: The OID of the commit to start at. We take parents of the
    /// provided commit until we end up at the target OID.
    /// * `target_oid`: The OID of the commit to end at.
    ///
    /// Returns: A path of commits from `commit_oid` through parents to `target_oid`.
    /// The path includes `commit_oid` at the beginning and `target_oid` at the end.
    /// If there is no such path, returns `None`.
    pub fn find_path_to_merge_base<'repo>(
        &self,
        effects: &Effects,
        repo: &'repo Repo,
        commit_oid: NonZeroOid,
        target_oid: NonZeroOid,
    ) -> eyre::Result<Option<Vec<Commit<'repo>>>> {
        let range = self.get_range(effects, repo, target_oid, commit_oid)?;
        let path = {
            let mut path = Vec::new();
            for oid in range {
                let commit = match repo.find_commit(oid)? {
                    Some(commit) => commit,
                    None => {
                        warn!("Commit in path to merge-base not found: {:?}", oid);
                        continue;
                    }
                };
                path.push(commit)
            }
            path
        };
        if path.is_empty() {
            Ok(None)
        } else {
            Ok(Some(path))
        }
    }
}

impl std::fmt::Debug for Dag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<Dag>")
    }
}
