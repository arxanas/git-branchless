//! Wrapper around the Eden SCM directed acyclic graph implementation, which
//! allows for efficient graph queries.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use eden_dag::ops::{DagPersistent, Parents};
use eden_dag::set::hints::Hints;
use eden_dag::{DagAlgorithm, Group, VertexListWithOptions, VertexOptions};
use eyre::Context;
use futures::{StreamExt, TryStreamExt};
use itertools::Itertools;
use once_cell::sync::OnceCell;
use tracing::{instrument, trace, warn};

use crate::core::effects::{Effects, OperationType};
use crate::core::eventlog::{CommitActivityStatus, EventCursor, EventReplayer};
use crate::git::{Commit, MaybeZeroOid, NonZeroOid, Repo, Time};

use super::repo_ext::RepoReferencesSnapshot;

impl From<NonZeroOid> for eden_dag::Vertex {
    fn from(oid: NonZeroOid) -> Self {
        eden_dag::Vertex::copy_from(oid.as_bytes())
    }
}

impl TryFrom<eden_dag::Vertex> for MaybeZeroOid {
    type Error = eyre::Error;

    fn try_from(value: eden_dag::Vertex) -> Result<Self, Self::Error> {
        let oid = git2::Oid::from_bytes(value.as_ref())?;
        let oid = MaybeZeroOid::from(oid);
        Ok(oid)
    }
}

impl TryFrom<eden_dag::Vertex> for NonZeroOid {
    type Error = eyre::Error;

    fn try_from(value: eden_dag::Vertex) -> Result<Self, Self::Error> {
        let oid = MaybeZeroOid::try_from(value)?;
        let oid = NonZeroOid::try_from(oid)?;
        Ok(oid)
    }
}

/// A compact set of commits, backed by the Eden DAG.
pub type CommitSet = eden_dag::Set;

/// A vertex referring to a single commit in the Eden DAG.
pub type CommitVertex = eden_dag::Vertex;

impl From<NonZeroOid> for CommitSet {
    fn from(oid: NonZeroOid) -> Self {
        let vertex = CommitVertex::from(oid);
        CommitSet::from_static_names([vertex])
    }
}

impl FromIterator<NonZeroOid> for CommitSet {
    fn from_iter<T: IntoIterator<Item = NonZeroOid>>(iter: T) -> Self {
        let oids = iter
            .into_iter()
            .map(CommitVertex::from)
            .map(Ok)
            .collect_vec();
        CommitSet::from_iter(oids, Hints::default())
    }
}

/// Union together a list of [CommitSet]s.
pub fn union_all(commits: &[CommitSet]) -> CommitSet {
    commits
        .iter()
        .fold(CommitSet::empty(), |acc, elem| acc.union(elem))
}

struct GitParentsBlocking {
    repo: Arc<Mutex<Repo>>,
}

#[async_trait]
impl Parents for GitParentsBlocking {
    async fn parent_names(&self, v: CommitVertex) -> eden_dag::Result<Vec<CommitVertex>> {
        use eden_dag::errors::BackendError;
        trace!(?v, "visiting Git commit");

        let oid = MaybeZeroOid::from_bytes(v.as_ref())
            .map_err(|_e| anyhow::anyhow!("Could not convert to Git oid: {:?}", &v))
            .map_err(BackendError::Other)?;
        let oid = match oid {
            MaybeZeroOid::NonZero(oid) => oid,
            MaybeZeroOid::Zero => return Ok(Vec::new()),
        };

        let repo = self.repo.lock().unwrap();
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
    }

    async fn hint_subdag_for_insertion(
        &self,
        _heads: &[CommitVertex],
    ) -> Result<eden_dag::MemDag, eden_dag::Error> {
        Ok(eden_dag::MemDag::new())
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
    observed_commits: CommitSet,

    /// A set containing all commits that have been determined to be obsolete by
    /// the `EventReplayer`.
    obsolete_commits: CommitSet,

    public_commits: OnceCell<CommitSet>,
    visible_heads: OnceCell<CommitSet>,
    visible_commits: OnceCell<CommitSet>,
    draft_commits: OnceCell<CommitSet>,
}

impl Dag {
    /// Reopen the DAG for the given repository.
    pub fn try_clone(&self, repo: &Repo) -> eyre::Result<Self> {
        let inner = Self::open_inner_dag(repo)?;
        Ok(Self {
            inner,
            head_commit: self.head_commit.clone(),
            main_branch_commit: self.main_branch_commit.clone(),
            branch_commits: self.branch_commits.clone(),
            observed_commits: self.observed_commits.clone(),
            obsolete_commits: self.obsolete_commits.clone(),
            public_commits: OnceCell::new(),
            visible_heads: OnceCell::new(),
            visible_commits: OnceCell::new(),
            draft_commits: OnceCell::new(),
        })
    }

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

        let obsolete_commits: CommitSet = observed_commits
            .iter()
            .copied()
            .filter(|commit_oid| {
                match event_replayer.get_cursor_commit_activity_status(event_cursor, *commit_oid) {
                    CommitActivityStatus::Active | CommitActivityStatus::Inactive => false,
                    CommitActivityStatus::Obsolete => true,
                }
            })
            .collect();

        let dag = Self::open_inner_dag(repo)?;

        let observed_commits: CommitSet = observed_commits.into_iter().collect();
        let head_commit = match head_oid {
            Some(head_oid) => CommitSet::from(*head_oid),
            None => CommitSet::empty(),
        };
        let main_branch_commit = CommitSet::from(*main_branch_oid);
        let branch_commits: CommitSet = branch_oid_to_names.keys().copied().collect();

        Ok(Self {
            inner: dag,
            head_commit,
            main_branch_commit,
            branch_commits,
            observed_commits,
            obsolete_commits,
            public_commits: Default::default(),
            visible_heads: Default::default(),
            visible_commits: Default::default(),
            draft_commits: Default::default(),
        })
    }

    #[instrument]
    fn open_inner_dag(repo: &Repo) -> eyre::Result<eden_dag::Dag> {
        let dag_dir = repo.get_dag_dir()?;
        std::fs::create_dir_all(&dag_dir).wrap_err("Creating .git/branchless/dag dir")?;
        let dag = eden_dag::Dag::open(&dag_dir)
            .wrap_err_with(|| format!("Opening DAG directory at: {:?}", &dag_dir))?;
        Ok(dag)
    }

    fn run_blocking<T>(&self, fut: impl Future<Output = T>) -> T {
        futures::executor::block_on(fut)
    }

    /// Update the DAG with all commits reachable from branches.
    #[instrument]
    fn sync(&mut self, effects: &Effects, repo: &Repo) -> eyre::Result<()> {
        let master_heads = self.main_branch_commit.clone();
        let non_master_heads = self
            .observed_commits
            .union(&self.head_commit)
            .union(&self.branch_commits);
        self.sync_from_oids(effects, repo, master_heads, non_master_heads)
    }

    /// Update the DAG with the given heads.
    #[instrument]
    pub fn sync_from_oids(
        &mut self,
        effects: &Effects,
        repo: &Repo,
        master_heads: CommitSet,
        non_master_heads: CommitSet,
    ) -> eyre::Result<()> {
        let (effects, _progress) = effects.start_operation(OperationType::UpdateCommitGraph);
        let _effects = effects;

        let master_group_options = {
            let mut options = VertexOptions::default();
            options.desired_group = Group::MASTER;
            options
        };
        let master_heads = self
            .commit_set_to_vec(&master_heads)?
            .into_iter()
            .map(|vertex| (CommitVertex::from(vertex), master_group_options.clone()))
            .collect_vec();
        let non_master_heads = self
            .commit_set_to_vec(&non_master_heads)?
            .into_iter()
            .map(|vertex| (CommitVertex::from(vertex), VertexOptions::default()))
            .collect_vec();
        let heads = [master_heads, non_master_heads].concat();

        let repo = repo.try_clone()?;
        futures::executor::block_on(self.inner.add_heads_and_flush(
            &GitParentsBlocking {
                repo: Arc::new(Mutex::new(repo)),
            },
            &VertexListWithOptions::from(heads),
        ))?;
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

    /// Create a new `Dag` with no obsolete commits.
    #[instrument]
    pub fn clear_obsolete_commits(&self, repo: &Repo) -> eyre::Result<Self> {
        let inner = Self::open_inner_dag(repo)?;
        Ok(Self {
            inner,
            head_commit: self.head_commit.clone(),
            branch_commits: self.branch_commits.clone(),
            main_branch_commit: self.main_branch_commit.clone(),
            observed_commits: self.observed_commits.clone(),
            obsolete_commits: CommitSet::empty(),
            draft_commits: Default::default(),
            public_commits: Default::default(),
            visible_heads: Default::default(),
            visible_commits: Default::default(),
        })
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn sort(&self, commit_set: &CommitSet) -> eyre::Result<Vec<NonZeroOid>> {
        let commit_set = self.run_blocking(self.inner.sort(commit_set))?;
        let commit_oids = self.commit_set_to_vec(&commit_set)?;

        // `.sort` seems to sort it such that the child-most commits are first?
        // In all current use-cases, we want to start with the parent commits.
        Ok(commit_oids.into_iter().rev().collect())
    }

    /// Eagerly convert a `CommitSet` into a `Vec<NonZeroOid>` by iterating over it, preserving order.
    #[instrument]
    pub fn commit_set_to_vec(&self, commit_set: &CommitSet) -> eyre::Result<Vec<NonZeroOid>> {
        async fn map_vertex(
            vertex: Result<CommitVertex, eden_dag::Error>,
        ) -> eyre::Result<NonZeroOid> {
            let vertex = vertex.wrap_err("Evaluating vertex")?;
            let vertex = NonZeroOid::try_from(vertex.clone())
                .wrap_err_with(|| format!("Converting vertex to NonZeroOid: {:?}", &vertex))?;
            Ok(vertex)
        }
        let stream = self
            .run_blocking(commit_set.iter())
            .wrap_err("Iterating commit set")?;
        let result = self.run_blocking(stream.then(map_vertex).try_collect())?;
        Ok(result)
    }

    /// Get the parent OID for the given OID. Returns an error if the given OID
    /// does not have exactly 1 parent.
    #[instrument]
    pub fn get_only_parent_oid(&self, oid: NonZeroOid) -> eyre::Result<NonZeroOid> {
        let parents: CommitSet = self.run_blocking(self.inner.parents(CommitSet::from(oid)))?;
        match self.commit_set_to_vec(&parents)?[..] {
            [oid] => Ok(oid),
            [] => Err(eyre::eyre!("Commit {} has no parents.", oid)),
            _ => Err(eyre::eyre!("Commit {} has more than 1 parents.", oid)),
        }
    }

    /// Conduct an arbitrary query against the DAG.
    pub fn query(&self) -> &eden_dag::Dag {
        &self.inner
    }

    /// Determine whether or not the given commit is a public commit (i.e. is an
    /// ancestor of the main branch).
    #[instrument]
    pub fn is_public_commit(&self, commit_oid: NonZeroOid) -> eyre::Result<bool> {
        let main_branch_commits = self.commit_set_to_vec(&self.main_branch_commit)?;
        for main_branch_commit in main_branch_commits {
            if self.run_blocking(
                self.inner
                    .is_ancestor(commit_oid.into(), main_branch_commit.into()),
            )? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_is_ancestor(
        &self,
        ancestor: NonZeroOid,
        descendant: NonZeroOid,
    ) -> eden_dag::Result<bool> {
        let result =
            self.run_blocking(self.inner.is_ancestor(ancestor.into(), descendant.into()))?;
        Ok(result)
    }

    /// Wrapper around NameSet method.
    #[instrument]
    pub fn set_is_empty(&self, commit_set: &CommitSet) -> eden_dag::Result<bool> {
        let result = self.run_blocking(commit_set.is_empty())?;
        Ok(result)
    }

    /// Wrapper around NameSet method.
    #[instrument]
    pub fn set_contains<T: Into<CommitVertex> + Debug>(
        &self,
        commit_set: &CommitSet,
        oid: T,
    ) -> eden_dag::Result<bool> {
        let result = self.run_blocking(commit_set.contains(&oid.into()))?;
        Ok(result)
    }

    /// Wrapper around NameSet method.
    #[instrument]
    pub fn set_count(&self, commit_set: &CommitSet) -> eden_dag::Result<usize> {
        let result = self.run_blocking(commit_set.count())?;
        Ok(result.try_into().unwrap())
    }

    /// Wrapper around NameSet method.
    #[instrument]
    pub fn set_first(&self, commit_set: &CommitSet) -> eden_dag::Result<Option<CommitVertex>> {
        let result = self.run_blocking(commit_set.first())?;
        Ok(result)
    }

    /// Return the set of commits which are public, as per the definition in
    /// `is_public_commit`. You should try to use `is_public_commit` instead, as
    /// it will be faster to compute.
    #[instrument]
    pub fn query_public_commits_slow(&self) -> eyre::Result<&CommitSet> {
        self.public_commits.get_or_try_init(|| {
            let public_commits =
                self.run_blocking(self.inner.ancestors(self.main_branch_commit.clone()))?;
            Ok(public_commits)
        })
    }

    /// Determine the set of commits which are considered to be "visible". A
    /// commit is "visible" if it is not obsolete or has a non-obsolete
    /// descendant.
    #[instrument]
    pub fn query_visible_heads(&self) -> eyre::Result<&CommitSet> {
        self.visible_heads.get_or_try_init(|| {
            let visible_heads = CommitSet::empty()
                .union(&self.observed_commits.difference(&self.obsolete_commits))
                .union(&self.head_commit)
                .union(&self.main_branch_commit)
                .union(&self.branch_commits);
            let visible_heads = self.run_blocking(self.inner.heads(visible_heads))?;
            Ok(visible_heads)
        })
    }

    /// Query the set of all visible commits, as per the definition in
    /// `query_visible_head`s. You should try to use `query_visible_heads`
    /// instead if possible, since it will be faster to compute.
    #[instrument]
    pub fn query_visible_commits_slow(&self) -> eyre::Result<&CommitSet> {
        self.visible_commits.get_or_try_init(|| {
            let visible_heads = self.query_visible_heads()?;
            let result = self.run_blocking(self.inner.ancestors(visible_heads.clone()))?;
            Ok(result)
        })
    }

    /// Keep only commits in the given set which are visible, as per the
    /// definition in `query_visible_heads`.
    #[instrument]
    pub fn filter_visible_commits(&self, commits: CommitSet) -> eyre::Result<CommitSet> {
        let visible_heads = self.query_visible_heads()?;
        Ok(commits.intersection(
            &self.run_blocking(self.inner.range(commits.clone(), visible_heads.clone()))?,
        ))
    }

    /// Determine the set of obsolete commits. These commits have been rewritten
    /// or explicitly hidden by the user.
    #[instrument]
    pub fn query_obsolete_commits(&self) -> CommitSet {
        self.obsolete_commits.clone()
    }

    /// Determine the set of "draft" commits. The draft commits are all visible
    /// commits which aren't public.
    #[instrument]
    pub fn query_draft_commits(&self) -> eyre::Result<&CommitSet> {
        self.draft_commits.get_or_try_init(|| {
            let visible_heads = self.query_visible_heads()?;
            let draft_commits = self.run_blocking(
                self.inner
                    .only(visible_heads.clone(), self.main_branch_commit.clone()),
            )?;
            Ok(draft_commits)
        })
    }

    /// Determine the connected components among draft commits (commit "stacks")
    /// that intersect with the provided set.
    #[instrument]
    pub fn query_stack_commits(&self, commit_set: CommitSet) -> eyre::Result<CommitSet> {
        let draft_commits = self.query_draft_commits()?;
        let stack_roots = self.query_roots(draft_commits.clone())?;
        let stack_ancestors = self.query_range(stack_roots, commit_set)?;
        let stack = self
            // Note that for a graph like
            //
            // ```
            // O
            // |
            // o A
            // | \
            // |  o B
            // |
            // @ C
            // ```
            // this will return `{A, B, C}`, not just `{A, C}`.
            .query_range(stack_ancestors, draft_commits.clone())?;
        Ok(stack)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_all(&self) -> eyre::Result<CommitSet> {
        let result = self.run_blocking(self.inner.all())?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_parents(&self, commit_set: CommitSet) -> eden_dag::Result<CommitSet> {
        let result = self.run_blocking(self.inner.parents(commit_set))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_parent_names<T: Into<CommitVertex> + Debug>(
        &self,
        vertex: T,
    ) -> eden_dag::Result<Vec<CommitVertex>> {
        let result = self.run_blocking(self.inner.parent_names(vertex.into()))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_ancestors(&self, commit_set: CommitSet) -> eden_dag::Result<CommitSet> {
        let result = self.run_blocking(self.inner.ancestors(commit_set))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_first_ancestor_nth(
        &self,
        vertex: CommitVertex,
        n: u64,
    ) -> eden_dag::Result<Option<CommitVertex>> {
        let result = self.run_blocking(self.inner.first_ancestor_nth(vertex, n))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_children(&self, commit_set: CommitSet) -> eden_dag::Result<CommitSet> {
        let result = self.run_blocking(self.inner.children(commit_set))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_descendants(&self, commit_set: CommitSet) -> eden_dag::Result<CommitSet> {
        let result = self.run_blocking(self.inner.descendants(commit_set))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_roots(&self, commit_set: CommitSet) -> eden_dag::Result<CommitSet> {
        let result = self.run_blocking(self.inner.roots(commit_set))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_heads(&self, commit_set: CommitSet) -> eden_dag::Result<CommitSet> {
        let result = self.run_blocking(self.inner.heads(commit_set))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_heads_ancestors(&self, commit_set: CommitSet) -> eden_dag::Result<CommitSet> {
        let result = self.run_blocking(self.inner.heads_ancestors(commit_set))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_only(
        &self,
        reachable: CommitSet,
        unreachable: CommitSet,
    ) -> eden_dag::Result<CommitSet> {
        let result = self.run_blocking(self.inner.only(reachable, unreachable))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_range(&self, roots: CommitSet, heads: CommitSet) -> eden_dag::Result<CommitSet> {
        let result = self.run_blocking(self.inner.range(roots, heads))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_common_ancestors(&self, commit_set: CommitSet) -> eden_dag::Result<CommitSet> {
        let result = self.run_blocking(self.inner.common_ancestors(commit_set))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_gca_one(&self, commit_set: CommitSet) -> eden_dag::Result<Option<CommitVertex>> {
        let result = self.run_blocking(self.inner.gca_one(commit_set))?;
        Ok(result)
    }

    /// Wrapper around DAG method.
    #[instrument]
    pub fn query_gca_all(&self, commit_set: CommitSet) -> eden_dag::Result<CommitSet> {
        let result = self.run_blocking(self.inner.gca_all(commit_set))?;
        Ok(result)
    }

    /// Given a CommitSet, return a list of CommitSets, each representing a
    /// connected component of the set.
    ///
    /// For example, if the DAG contains commits A-B-C-D-E-F and the given
    /// CommitSet contains `B, C, E`, this will return 2 `CommitSet`s: 1
    /// containing `B, C` and another containing only `E`
    #[instrument]
    pub fn get_connected_components(&self, commit_set: &CommitSet) -> eyre::Result<Vec<CommitSet>> {
        let mut components: Vec<CommitSet> = Vec::new();
        let mut component = CommitSet::empty();
        let mut commits_to_connect = commit_set.clone();

        // FIXME: O(n^2) algorithm (
        // FMI see https://github.com/arxanas/git-branchless/pull/450#issuecomment-1188391763
        for commit in self.commit_set_to_vec(commit_set)? {
            if self.run_blocking(commits_to_connect.is_empty())? {
                break;
            }

            if !self.run_blocking(commits_to_connect.contains(&commit.into()))? {
                continue;
            }

            let mut commits = CommitSet::from(commit);
            while !self.run_blocking(commits.is_empty())? {
                component = component.union(&commits);
                commits_to_connect = commits_to_connect.difference(&commits);

                let parents = self.run_blocking(self.inner.parents(commits.clone()))?;
                let children = self.run_blocking(self.inner.children(commits.clone()))?;
                commits = parents.union(&children).intersection(&commits_to_connect);
            }

            components.push(component);
            component = CommitSet::empty();
        }

        let connected_commits = union_all(&components);
        assert_eq!(
            self.run_blocking(commit_set.count())?,
            self.run_blocking(connected_commits.count())?
        );
        let connected_commits = commit_set.intersection(&connected_commits);
        assert_eq!(
            self.run_blocking(commit_set.count())?,
            self.run_blocking(connected_commits.count())?
        );

        Ok(components)
    }
}

impl std::fmt::Debug for Dag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<Dag>")
    }
}

/// Sort the given set of commits topologically.
///
/// In the case of two commits being unorderable, sort them using a
/// deterministic tie-breaking function. Commits which have been garbage
/// collected and are no longer available in the repository are omitted.
///
/// FIXME: this function does not use a total ordering for the sort, which could
/// mean that it produces incorrect results. Suppose that we have a graph with
/// parentage relationships A < B, B < C, A < D. Since D is not directly
/// comparable with B or C, it's possible that we calculate D < B and D > C,
/// which violates transitivity (D < B and B < C implies that D < C).
///
/// We only use this function to produce deterministic output, so in practice,
/// it doesn't seem to have a serious impact.
pub fn sorted_commit_set<'repo>(
    repo: &'repo Repo,
    dag: &Dag,
    commit_set: &CommitSet,
) -> eyre::Result<Vec<Commit<'repo>>> {
    let commit_oids = dag.commit_set_to_vec(commit_set)?;
    let mut commits: Vec<Commit> = {
        let mut commits = Vec::new();
        for commit_oid in commit_oids {
            if let Some(commit) = repo.find_commit(commit_oid)? {
                commits.push(commit)
            }
        }
        commits
    };

    let commit_times: HashMap<NonZeroOid, Time> = commits
        .iter()
        .map(|commit| (commit.get_oid(), commit.get_time()))
        .collect();

    commits.sort_by(|lhs, rhs| {
        let lhs_vertex = CommitVertex::from(lhs.get_oid());
        let rhs_vertex = CommitVertex::from(rhs.get_oid());
        if dag
            .query_is_ancestor(lhs.get_oid(), rhs.get_oid())
            .unwrap_or_else(|_| {
                warn!(
                    ?lhs_vertex,
                    ?rhs_vertex,
                    "Could not calculate `is_ancestor`"
                );
                false
            })
        {
            return Ordering::Less;
        } else if dag
            .query_is_ancestor(rhs.get_oid(), lhs.get_oid())
            .unwrap_or_else(|_| {
                warn!(
                    ?lhs_vertex,
                    ?rhs_vertex,
                    "Could not calculate `is_ancestor`"
                );
                false
            })
        {
            return Ordering::Greater;
        }

        (&commit_times[&lhs.get_oid()], lhs.get_oid())
            .cmp(&(&commit_times[&rhs.get_oid()], rhs.get_oid()))
    });

    Ok(commits)
}
