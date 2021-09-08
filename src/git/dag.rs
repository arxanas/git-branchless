use std::borrow::Borrow;
use std::convert::TryFrom;

use eden_dag::ops::DagPersistent;
use eden_dag::DagAlgorithm;
use eyre::Context;
use itertools::Itertools;
use tracing::{instrument, trace, warn};

use crate::core::eventlog::{CommitActivityStatus, EventCursor, EventReplayer};
use crate::git::{Commit, MaybeZeroOid, NonZeroOid, Repo};
use crate::tui::{Effects, OperationType};

use super::RepoReferencesSnapshot;

impl From<NonZeroOid> for eden_dag::VertexName {
    fn from(oid: NonZeroOid) -> Self {
        eden_dag::VertexName::copy_from(oid.as_bytes())
    }
}

impl TryFrom<eden_dag::VertexName> for NonZeroOid {
    type Error = eyre::Error;

    fn try_from(value: eden_dag::VertexName) -> Result<Self, Self::Error> {
        let oid = git2::Oid::from_bytes(value.as_ref())?;
        let oid = MaybeZeroOid::from(oid);
        NonZeroOid::try_from(oid)
    }
}

/// A compact set of commits, backed by the Eden DAG.
pub type CommitSet = eden_dag::NameSet;

/// A vertex referring to a single commit in the Eden DAG.
pub type CommitVertex = eden_dag::VertexName;

impl From<NonZeroOid> for CommitSet {
    fn from(oid: NonZeroOid) -> Self {
        let vertex = CommitVertex::from(oid);
        CommitSet::from_static_names([vertex])
    }
}

/// Interface to access the directed acyclic graph (DAG) representing Git's
/// commit graph. Based on the Eden SCM DAG.
pub struct Dag {
    inner: eden_dag::Dag,

    /// A set containing the commit which `HEAD` points to. If `HEAD` is unborn,
    /// this is an empty set.
    pub head_commit: CommitSet,

    /// A set containing the commit that the main branch currently points to.
    pub main_branch_commit: CommitSet,

    /// A set containing all commits currently pointed to by local branches.
    pub branch_commits: CommitSet,

    /// A set containing all commits that have been observed by the
    /// `EventReplayer`.
    pub observed_commits: CommitSet,

    /// A set containing all commits that have been determined to be obsolete by
    /// the `EventReplayer`.
    pub obsolete_commits: CommitSet,
}

impl Dag {
    /// Initialize the DAG for the given repository, and update it with any
    /// newly-referenced commits.
    #[instrument]
    pub fn open_and_sync(
        effects: &Effects,
        repo: &Repo,
        event_replayer: &EventReplayer,
        event_cursor: EventCursor,
        references_snapshot: &RepoReferencesSnapshot,
    ) -> eyre::Result<Self> {
        let mut dag = Self::open_without_syncing(
            effects,
            repo,
            event_replayer,
            event_cursor,
            references_snapshot,
        )?;
        dag.sync(effects, repo)?;
        Ok(dag)
    }

    /// Initialize a DAG for the given repository, without updating it with new
    /// commits that may have appeared.
    ///
    /// If used improperly, commit lookups could fail at runtime. This function
    /// should only be used for opening the DAG when it's known that no more
    /// live commits have appeared.
    #[instrument]
    pub fn open_without_syncing(
        effects: &Effects,
        repo: &Repo,
        event_replayer: &EventReplayer,
        event_cursor: EventCursor,
        references_snapshot: &RepoReferencesSnapshot,
    ) -> eyre::Result<Self> {
        let observed_commits = event_replayer.get_cursor_oids(event_cursor);
        let RepoReferencesSnapshot {
            head_oid,
            main_branch_oid,
            branch_oid_to_names,
        } = references_snapshot;

        let obsolete_commits = CommitSet::from_iter(
            observed_commits
                .iter()
                .copied()
                .filter(|commit_oid| {
                    match event_replayer
                        .get_cursor_commit_activity_status(event_cursor, *commit_oid)
                    {
                        CommitActivityStatus::Active | CommitActivityStatus::Inactive => false,
                        CommitActivityStatus::Obsolete => true,
                    }
                })
                .map(CommitVertex::from)
                .map(Ok)
                .collect_vec(),
        );

        let dag_dir = repo.get_dag_dir()?;
        let dag = eden_dag::Dag::open(&dag_dir)
            .wrap_err_with(|| format!("Opening DAG directory at: {:?}", &dag_dir))?;

        let observed_commits =
            CommitSet::from_iter(observed_commits.into_iter().map(CommitVertex::from).map(Ok));
        let head_commit = match head_oid {
            Some(head_oid) => CommitSet::from(*head_oid),
            None => CommitSet::empty(),
        };
        let main_branch_commit = CommitSet::from(*main_branch_oid);
        let branch_commits = CommitSet::from_iter(
            branch_oid_to_names
                .keys()
                .copied()
                .map(CommitVertex::from)
                .map(Ok)
                .collect_vec(),
        );

        Ok(Self {
            inner: dag,
            head_commit,
            main_branch_commit,
            branch_commits,
            observed_commits,
            obsolete_commits,
        })
    }

    /// This function's code adapted from `GitDag`, licensed under GPL-2.
    fn sync(&mut self, effects: &Effects, repo: &Repo) -> eden_dag::Result<()> {
        let master_heads = self.main_branch_commit.clone();
        let non_master_heads = self
            .observed_commits
            .union(&self.head_commit)
            .union(&self.branch_commits);
        self.sync_from_oids(effects, repo, master_heads, non_master_heads)
    }

    /// Update the DAG with the given heads.
    pub fn sync_from_oids(
        &mut self,
        effects: &Effects,
        repo: &Repo,
        master_heads: CommitSet,
        non_master_heads: CommitSet,
    ) -> eden_dag::Result<()> {
        let (effects, _progress) = effects.start_operation(OperationType::UpdateCommitGraph);
        let _effects = effects;

        let parent_func = |v: CommitVertex| -> eden_dag::Result<Vec<CommitVertex>> {
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
                .map(CommitVertex::from)
                .collect())
        };

        let commit_set_to_vec = |commit_set: CommitSet| -> Vec<CommitVertex> {
            let mut result = Vec::new();
            for vertex in commit_set
                .iter()
                .expect("The commit set was produced statically, so iteration should not fail")
            {
                let vertex = vertex.expect(
                    "The commit set was produced statically, so accessing a vertex should not fail",
                );
                result.push(vertex);
            }
            result
        };
        self.inner.add_heads_and_flush(
            parent_func,
            commit_set_to_vec(master_heads).as_slice(),
            commit_set_to_vec(non_master_heads).as_slice(),
        )?;
        Ok(())
    }

    /// Create a new version of this DAG at the point in time represented by
    /// `event_cursor`.
    pub fn set_cursor(
        &self,
        effects: &Effects,
        repo: &Repo,
        event_replayer: &EventReplayer,
        event_cursor: EventCursor,
    ) -> eyre::Result<Self> {
        let references_snapshot = event_replayer.get_references_snapshot(repo, event_cursor)?;
        let dag = Self::open_without_syncing(
            effects,
            repo,
            event_replayer,
            event_cursor,
            &references_snapshot,
        )?;
        Ok(dag)
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
        let set = vec![CommitVertex::from(lhs_oid), CommitVertex::from(rhs_oid)];
        let set = self
            .inner
            .sort(&CommitSet::from_static_names(set))
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
        let roots = CommitSet::from_static_names(vec![CommitVertex::from(parent_oid)]);
        let heads = CommitSet::from_static_names(vec![CommitVertex::from(child_oid)]);
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

    /// Conduct an arbitrary query against the DAG.
    pub fn query(&self) -> &eden_dag::Dag {
        &*self.inner.borrow()
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

    /// Find a path from the provided head to its merge-base with the main
    /// branch.
    #[instrument]
    pub fn find_path_to_main_branch(
        &self,
        effects: &Effects,
        head: CommitSet,
    ) -> eyre::Result<Option<CommitSet>> {
        // FIXME: this assumes that there is only one merge-base with the main branch.
        let merge_base = {
            let (_effects, _progress) = effects.start_operation(OperationType::GetMergeBase);
            self.query().gca_one(self.main_branch_commit.union(&head))?
        };
        let merge_base = match merge_base {
            Some(merge_base) => merge_base,
            None => return Ok(None),
        };

        // FIXME: this assumes that there is only one path to the merge-base.
        let path = {
            let (_effects, _progress) = effects.start_operation(OperationType::FindPathToMergeBase);
            self.query().range(CommitSet::from(merge_base), head)?
        };
        Ok(Some(path))
    }
}

impl std::fmt::Debug for Dag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<Dag>")
    }
}
