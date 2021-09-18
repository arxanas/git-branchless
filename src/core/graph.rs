//! Renders the smartlog commit graph based on user activity.
//!
//! This is the basic data structure that most of branchless operates on.

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::ops::Deref;

use tracing::{instrument, warn};

use crate::core::eventlog::{CommitActivityStatus, Event, EventCursor, EventReplayer};
use crate::git::{Commit, Dag, NonZeroOid, Repo, RepoReferencesSnapshot};
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

    /// The latest event to affect this commit.
    ///
    /// It's possible that no event affected this commit, and it was simply
    /// visible due to a branch pointing to it. In that case, this field is
    /// `None`.
    pub event: Option<Event>,
}

/// Graph of commits that the user is working on.
#[derive(Default)]
pub struct CommitGraph<'repo> {
    nodes: HashMap<NonZeroOid, Node<'repo>>,
}

impl std::fmt::Debug for CommitGraph<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<CommitGraph len={}>", self.nodes.len())
    }
}

impl<'repo> Deref for CommitGraph<'repo> {
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
#[instrument(skip(commit_oids))]
fn walk_from_commits<'repo>(
    effects: &Effects,
    repo: &'repo Repo,
    dag: &Dag,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    main_branch_oid: NonZeroOid,
    commit_oids: &HashSet<NonZeroOid>,
) -> eyre::Result<CommitGraph<'repo>> {
    let (effects, _progress) = effects.start_operation(OperationType::WalkCommits);

    let mut graph: HashMap<NonZeroOid, Node> = Default::default();

    for commit_oid in commit_oids {
        let commit = repo.find_commit(*commit_oid)?;
        let current_commit = match commit {
            Some(commit) => commit,

            // Commit may have been garbage-collected.
            None => continue,
        };

        let merge_base_oid =
            dag.get_one_merge_base_oid(&effects, repo, current_commit.get_oid(), main_branch_oid)?;
        let path_to_merge_base = match merge_base_oid {
            // Occasionally we may find a commit that has no merge-base with the
            // main branch. For example: a rewritten initial commit. This is
            // somewhat pathological. We'll just add it to the graph as a
            // standalone component and hope it works out.
            None => vec![current_commit],
            Some(merge_base_oid) => {
                let path_to_merge_base = dag.find_path_to_merge_base(
                    &effects,
                    repo,
                    current_commit.get_oid(),
                    merge_base_oid,
                )?;
                match path_to_merge_base {
                    None => {
                        warn!(
                            current_commit_oid = ?current_commit.get_oid(),
                            "No path to merge-base for commit",
                        );
                        continue;
                    }
                    Some(path_to_merge_base) => path_to_merge_base,
                }
            }
        };

        for current_commit in path_to_merge_base.iter() {
            if graph.contains_key(&current_commit.get_oid()) {
                // This commit (and all of its parents!) should be in the graph
                // already, so no need to continue this iteration.
                break;
            }

            let activity_status = event_replayer
                .get_cursor_commit_activity_status(event_cursor, current_commit.get_oid());
            let is_obsolete = match activity_status {
                CommitActivityStatus::Obsolete => true,
                CommitActivityStatus::Active | CommitActivityStatus::Inactive => false,
            };

            let is_main = match merge_base_oid {
                Some(merge_base_oid) => (current_commit.get_oid() == merge_base_oid),
                None => false,
            };

            let event = event_replayer
                .get_cursor_commit_latest_event(event_cursor, current_commit.get_oid())
                .cloned();
            graph.insert(
                current_commit.get_oid(),
                Node {
                    commit: current_commit.clone(),
                    parent: None,
                    children: Vec::new(),
                    is_main,
                    is_obsolete,
                    event,
                },
            );
        }

        if let Some(merge_base_oid) = merge_base_oid {
            if !graph.contains_key(&merge_base_oid) {
                warn!(?merge_base_oid, "Could not find merge base OID");
            }
        }
    }

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

    Ok(CommitGraph { nodes: graph })
}

/// Sort children nodes of the commit graph in a standard order, for determinism
/// in output.
fn sort_children(graph: &mut CommitGraph) {
    let commit_times: HashMap<NonZeroOid, git2::Time> = graph
        .iter()
        .map(|(oid, node)| (*oid, node.commit.get_time()))
        .collect();
    for node in graph.nodes.values_mut() {
        node.children
            .sort_by_key(|child_oid| (commit_times[child_oid], child_oid.to_string()));
    }
}

fn is_commit_visible(
    cache: &mut HashMap<NonZeroOid, bool>,
    graph: &CommitGraph,
    unhideable_oids: &HashSet<NonZeroOid>,
    oid: &NonZeroOid,
) -> bool {
    if let Some(result) = cache.get(oid) {
        return *result;
    }

    if unhideable_oids.contains(oid) {
        return true;
    }

    let result = {
        let node = &graph[oid];
        match node {
            Node {
                commit: _,
                parent: _,
                children: _,
                is_main: false,
                is_obsolete: false,
                event: _,
            } => {
                // This is an active commit.
                true
            }

            Node {
                commit: _,
                parent: _,
                children,
                is_main: false,
                is_obsolete: true,
                event: _,
            } => {
                // This is an obsolete commit, so show it only if it has a visible descendant.
                children
                    .iter()
                    .any(|child_oid| is_commit_visible(cache, graph, unhideable_oids, child_oid))
            }

            Node {
                commit: _,
                parent: _,
                children,
                is_main: true,
                is_obsolete: false,
                event: _,
            } => {
                // Main branch commits are not interesting by default. Only show
                // it if it has an active child. But don't consider any visible
                // children which are also main branch commits.
                children
                    .iter()
                    // Don't consider the next commit in the main branch as a
                    // descendant for visibility-calculation purposes.
                    .filter(|child_oid| !graph[child_oid].is_main)
                    .any(|child_oid| is_commit_visible(cache, graph, unhideable_oids, child_oid))
            }

            Node {
                commit: _,
                parent: _,
                children: _,
                is_main: true,
                is_obsolete: true,
                event: _,
            } => {
                // An obsolete main branch commit is an anomaly, so surface it
                // for the user.
                true
            }
        }
    };

    cache.insert(*oid, result);
    result
}

/// Remove hidden commits from the graph.
fn do_remove_commits(graph: &mut CommitGraph, references_snapshot: &RepoReferencesSnapshot) {
    // OIDs which are pointed to by HEAD or a branch should not be hidden.
    // Therefore, we can't hide them *or* their ancestors.
    let mut unhideable_oids: HashSet<NonZeroOid> = references_snapshot
        .branch_oid_to_names
        .keys()
        .copied()
        .collect();
    if let Some(head_oid) = references_snapshot.head_oid {
        unhideable_oids.insert(head_oid);
    }

    let mut cache = HashMap::new();
    let all_hidden_oids: HashSet<NonZeroOid> = graph
        .keys()
        .filter(|oid| !is_commit_visible(&mut cache, graph, &unhideable_oids, oid))
        .cloned()
        .collect();

    // Actually update the graph and delete any parent-child links, as
    // appropriate.
    for oid in all_hidden_oids {
        let parent_oid = graph[&oid].parent;
        graph.nodes.remove(&oid);
        match parent_oid {
            Some(parent_oid) if graph.contains_key(&parent_oid) => {
                let children = &mut graph.nodes.get_mut(&parent_oid).unwrap().children;
                *children = children
                    .iter()
                    .filter_map(|child_oid| {
                        if *child_oid != oid {
                            Some(*child_oid)
                        } else {
                            None
                        }
                    })
                    .collect();
            }
            _ => {}
        }
    }
}

/// Construct the smartlog graph for the repo.
///
/// Args:
/// * `repo`: The Git repository.
/// * `merge_base_db`: The merge-base database.
/// * `event_replayer`: The event replayer.
/// * `head_oid`: The OID of the repository's `HEAD` reference.
/// * `main_branch_oid`: The OID of the main branch.
/// * `branch_oids`: The set of OIDs pointed to by branches.
/// * `hide_commits`: If set to `True`, then, after constructing the graph,
/// remove nodes from it that appear to be hidden by user activity. This should
/// be set to `True` for most display-related purposes.
///
/// Returns: A tuple of the head OID and the commit graph.
#[instrument]
pub fn make_graph<'repo>(
    effects: &Effects,
    repo: &'repo Repo,
    dag: &Dag,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    references_snapshot: &RepoReferencesSnapshot,
    remove_commits: bool,
) -> eyre::Result<CommitGraph<'repo>> {
    let (effects, _progress) = effects.start_operation(OperationType::MakeGraph);

    let mut commit_oids: HashSet<NonZeroOid> = event_replayer
        .get_cursor_oids(event_cursor)
        .into_iter()
        .collect();
    commit_oids.extend(references_snapshot.branch_oid_to_names.keys());
    if let Some(head_oid) = references_snapshot.head_oid {
        commit_oids.insert(head_oid);
    }
    let mut graph = walk_from_commits(
        &effects,
        repo,
        dag,
        event_replayer,
        event_cursor,
        references_snapshot.main_branch_oid,
        &commit_oids,
    )?;
    sort_children(&mut graph);
    if remove_commits {
        do_remove_commits(&mut graph, references_snapshot);
    }
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
