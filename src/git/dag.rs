use std::cell::RefCell;
use std::collections::HashSet;

use eden_dag::ops::DagPersistent;
use eden_dag::{DagAlgorithm, Set, Vertex};
use eyre::Context;
use itertools::Itertools;
use tracing::{instrument, trace};

use crate::core::eventlog::EventReplayer;
use crate::core::mergebase::MergeBaseDb;
use crate::git::{MaybeZeroOid, NonZeroOid, Repo};
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
            Self::update_oids(effects, repo, &mut dag, master_oids, non_master_oids)?;
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
        master_oids: impl IntoIterator<Item = NonZeroOid>,
        non_master_oids: impl IntoIterator<Item = NonZeroOid>,
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
                .into_iter()
                .map(|oid| Vertex::copy_from(oid.as_bytes()))
                .collect_vec()
                .as_slice(),
            non_master_oids
                .into_iter()
                .map(|oid| Vertex::copy_from(oid.as_bytes()))
                .collect_vec()
                .as_slice(),
        )?;
        Ok(())
    }

    #[instrument]
    fn oid_to_vertex(
        &self,
        effects: &Effects,
        repo: &Repo,
        dag: &mut eden_dag::Dag,
        oid: NonZeroOid,
    ) -> eyre::Result<Vertex> {
        Self::update_oids(effects, repo, dag, Vec::new(), vec![oid])?;
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
            .sort(&Set::from_static_names(set.clone()))
            .wrap_err_with(|| format!("Sorting DAG vertex set: {:?}", &set))?;
        let vertex = dag.gca_one(set).wrap_err_with(|| {
            format!(
                "Computing merge-base between {:?} and {:?}",
                lhs_oid, rhs_oid
            )
        })?;
        match vertex {
            None => Ok(None),
            Some(vertex) => Ok(Some(vertex.to_hex().parse()?)),
        }
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
}
