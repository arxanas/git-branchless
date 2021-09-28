//! Renders the smartlog commit graph based on user activity.
//!
//! This is the basic data structure that most of branchless operates on.

use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::Debug;
use std::ops::Deref;

use eden_dag::DagAlgorithm;
use tracing::instrument;

use crate::core::eventlog::{EventCursor, EventReplayer};
use crate::git::{Commit, CommitSet, Dag, NonZeroOid, Repo};
use crate::tui::{Effects, OperationType};

/// Node contained in the smartlog commit graph.
#[derive(Debug)]
pub struct Node<'repo> {
    /// The underlying commit object.
    pub commit: Commit<'repo>,

    /// The OID of the parent node in the smartlog commit graph.
    ///
    /// This is different from inspecting `commit.parents()`, since the smartlog
    /// will hide most nodes from the commit graph, including parent nodes.
    pub parent: Option<NonZeroOid>,

    /// The OIDs of the children nodes in the smartlog commit graph.
    pub children: Vec<NonZeroOid>,

    /// Indicates that this is a commit to the main branch.
    ///
    /// These commits are considered to be immutable and should never leave the
    /// `main` state. But this can still happen in practice if the user's
    /// workflow is different than expected.
    pub is_main: bool,

    /// Indicates that this commit has been marked as obsolete.
    ///
    /// Commits are marked as obsolete when they've been rewritten into another
    /// commit, or explicitly marked such by the user. Normally, they're not
    /// visible in the smartlog, except if there's some anomalous situation that
    /// the user should take note of (such as an obsolete commit having a
    /// non-obsolete descendant).
    ///
    /// Occasionally, a main commit can be marked as obsolete, such as if a
    /// commit in the main branch has been rewritten. We don't expect this to
    /// happen in the monorepo workflow, but it can happen in other workflows
    /// where you commit directly to the main branch and then later rewrite the
    /// commit.
    pub is_obsolete: bool,
}

/// Graph of commits that the user is working on.
#[derive(Default)]
pub struct SmartlogGraph<'repo> {
    nodes: HashMap<NonZeroOid, Node<'repo>>,
}

impl std::fmt::Debug for SmartlogGraph<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<CommitGraph len={}>", self.nodes.len())
    }
}

impl<'repo> Deref for SmartlogGraph<'repo> {
    type Target = HashMap<NonZeroOid, Node<'repo>>;

    fn deref(&self) -> &Self::Target {
        &self.nodes
    }
}

/// Find additional commits that should be displayed.
///
/// For example, if you check out a commit that has intermediate parent commits
/// between it and the main branch, those intermediate commits should be shown
/// (or else you won't get a good idea of the line of development that happened
/// for this commit since the main branch).
#[instrument]
fn find_visible_commits<'repo>(
    effects: &Effects,
    repo: &'repo Repo,
    dag: &Dag,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    visible_heads: &CommitSet,
) -> eyre::Result<SmartlogGraph<'repo>> {
    let public_commits = dag.query().ancestors(dag.main_branch_commit.clone())?;

    let mut graph: HashMap<NonZeroOid, Node> = {
        let mut result = HashMap::new();
        for vertex in visible_heads.iter()? {
            let vertex = vertex?;
            let path_to_main_branch =
                dag.find_path_to_main_branch(effects, CommitSet::from(vertex.clone()))?;
            let path_to_main_branch = match path_to_main_branch {
                Some(path_to_main_branch) => path_to_main_branch,
                None => CommitSet::from(vertex.clone()),
            };

            for vertex in path_to_main_branch.iter_rev()? {
                let vertex = vertex?;
                let oid = NonZeroOid::try_from(vertex.clone())?;

                let commit = match repo.find_commit(oid)? {
                    Some(commit) => commit,
                    None => {
                        // This commit may have been garbage collected.
                        continue;
                    }
                };

                result.insert(
                    commit.get_oid(),
                    Node {
                        commit,
                        parent: None,         // populated below
                        children: Vec::new(), // populated below
                        is_main: public_commits.contains(&vertex)?,
                        is_obsolete: dag.obsolete_commits.contains(&vertex)?,
                    },
                );
            }
        }
        result
    };

    // Find immediate parent-child links.
    let links: Vec<(NonZeroOid, NonZeroOid)> = graph
        .iter()
        .filter(|(_child_oid, node)| !node.is_main)
        .flat_map(|(child_oid, node)| {
            node.commit
                .get_parent_oids()
                .into_iter()
                .filter(|parent_oid| graph.contains_key(parent_oid))
                .map(move |parent_oid| (*child_oid, parent_oid))
        })
        .collect();
    for (child_oid, parent_oid) in links.iter() {
        graph.get_mut(child_oid).unwrap().parent = Some(*parent_oid);
        graph.get_mut(parent_oid).unwrap().children.push(*child_oid);
    }

    Ok(SmartlogGraph { nodes: graph })
}

/// Sort children nodes of the commit graph in a standard order, for determinism
/// in output.
fn sort_children(graph: &mut SmartlogGraph) {
    let commit_times: HashMap<NonZeroOid, git2::Time> = graph
        .iter()
        .map(|(oid, node)| (*oid, node.commit.get_time()))
        .collect();
    for node in graph.nodes.values_mut() {
        node.children
            .sort_by_key(|child_oid| (commit_times[child_oid], child_oid.to_string()));
    }
}

/// Construct the smartlog graph for the repo.
#[instrument]
pub fn make_smartlog_graph<'repo>(
    effects: &Effects,
    repo: &'repo Repo,
    dag: &Dag,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    remove_commits: bool,
) -> eyre::Result<SmartlogGraph<'repo>> {
    let (effects, _progress) = effects.start_operation(OperationType::MakeGraph);

    let mut graph = {
        let (effects, _progress) = effects.start_operation(OperationType::WalkCommits);

        // This is a large, lazy set. Be sure not to force evaluation of the
        // entire set.
        let public_commits = &dag.query().ancestors(dag.main_branch_commit.clone())?;

        let visible_heads = if remove_commits {
            dag.observed_commits.difference(&dag.obsolete_commits)
        } else {
            dag.observed_commits.clone()
        };
        let visible_heads = dag.query().heads(visible_heads)?;
        let visible_heads = visible_heads.difference(public_commits);

        let anomalous_main_branch_commits = dag.obsolete_commits.intersection(public_commits);
        let visible_heads = visible_heads
            .union(&dag.head_commit)
            .union(&dag.branch_commits)
            .union(&dag.main_branch_commit)
            .union(&anomalous_main_branch_commits);

        find_visible_commits(
            &effects,
            repo,
            dag,
            event_replayer,
            event_cursor,
            &visible_heads,
        )?
    };
    sort_children(&mut graph);
    Ok(graph)
}

/// The result of attempting to resolve commits.
pub enum ResolveCommitsResult<'repo> {
    /// All commits were successfully resolved.
    Ok {
        /// The commits.
        commits: Vec<Commit<'repo>>,
    },

    /// The first commit which couldn't be resolved.
    CommitNotFound {
        /// The identifier of the commit, as provided by the user.
        commit: String,
    },
}

/// Parse strings which refer to commits, such as:
///
/// - Full OIDs.
/// - Short OIDs.
/// - Reference names.
#[instrument]
pub fn resolve_commits(repo: &Repo, hashes: Vec<String>) -> eyre::Result<ResolveCommitsResult> {
    let mut commits = Vec::new();
    for hash in hashes {
        let commit = match repo.revparse_single_commit(&hash)? {
            Some(commit) => commit,
            None => return Ok(ResolveCommitsResult::CommitNotFound { commit: hash }),
        };
        commits.push(commit)
    }
    Ok(ResolveCommitsResult::Ok { commits })
}
