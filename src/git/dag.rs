use std::cell::RefCell;
use std::collections::HashSet;

use eden_dag::ops::DagPersistent;
use eden_dag::{DagAlgorithm, Set, Vertex};
use eyre::Context;
use itertools::Itertools;
use tracing::{instrument, trace, warn};

use crate::core::eventlog::EventReplayer;
use crate::core::mergebase::MergeBaseDb;
use crate::git::{Commit, MaybeZeroOid, NonZeroOid, Repo};
use crate::tui::{Effects, OperationType};

/// Interface to access the directed acyclic graph (DAG) representing Git's
/// commit graph. Based on the Eden SCM DAG.
pub struct Dag {
    inner: RefCell<eden_dag::Dag>,
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
        let active_oids = event_replayer.get_cursor_active_oids(event_cursor);
        let main_branch_oid = repo.get_main_branch_oid()?;

        let master_oids = {
            let mut result = HashSet::new();
            result.insert(main_branch_oid);
            result
        };
        let non_master_oids = {
            let mut result = active_oids;
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

        Ok(Self {
            inner: RefCell::new(dag),
        })
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
                .map(|oid| Vertex::copy_from(oid.as_bytes()))
                .collect())
        };

        dag.add_heads_and_flush(
            parent_func,
            master_oids
                .iter()
                .map(|oid| Vertex::copy_from(oid.as_bytes()))
                .collect_vec()
                .as_slice(),
            non_master_oids
                .iter()
                .map(|oid| Vertex::copy_from(oid.as_bytes()))
                .collect_vec()
                .as_slice(),
        )?;
        Ok(())
    }

    #[instrument(skip(dag))]
    fn oid_to_vertex(
        &self,
        effects: &Effects,
        repo: &Repo,
        dag: &mut eden_dag::Dag,
        oid: NonZeroOid,
    ) -> eyre::Result<Vertex> {
        let master_oids = HashSet::new();
        let non_master_oids = {
            let mut non_master_oids = HashSet::new();
            non_master_oids.insert(oid);
            non_master_oids
        };
        Self::update_oids(effects, repo, dag, &master_oids, &non_master_oids)?;
        Ok(Vertex::copy_from(oid.as_bytes()))
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
        let mut dag = self.inner.try_borrow_mut()?;
        let dag = &mut *dag;
        let set = vec![
            self.oid_to_vertex(effects, repo, dag, lhs_oid)?,
            self.oid_to_vertex(effects, repo, dag, rhs_oid)?,
        ];
        let set = dag
            .sort(&Set::from_static_names(set))
            .wrap_err("Sorting DAG vertex set")?;
        let vertex = dag.gca_one(set).wrap_err("Computing merge-base")?;
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
        let mut dag = self.inner.try_borrow_mut()?;
        let dag = &mut *dag;
        let roots =
            Set::from_static_names(vec![self.oid_to_vertex(effects, repo, dag, parent_oid)?]);
        let heads =
            Set::from_static_names(vec![self.oid_to_vertex(effects, repo, dag, child_oid)?]);
        let range = dag.range(roots, heads).wrap_err("Computing range")?;
        let range = dag.sort(&range).wrap_err("Sorting range")?;
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
}

impl std::fmt::Debug for Dag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<Dag>")
    }
}

impl MergeBaseDb for Dag {
    fn get_merge_base_oid(
        &self,
        effects: &Effects,
        repo: &Repo,
        lhs_oid: NonZeroOid,
        rhs_oid: NonZeroOid,
    ) -> eyre::Result<Option<NonZeroOid>> {
        self.get_one_merge_base_oid(effects, repo, lhs_oid, rhs_oid)
    }

    fn find_path_to_merge_base<'repo>(
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
