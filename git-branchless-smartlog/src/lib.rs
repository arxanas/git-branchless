//! Display a graph of commits that the user has worked on recently.
//!
//! The set of commits that are still being worked on is inferred from the event
//! log; see the `eventlog` module.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_conditions)]

use std::cmp::Ordering;
use std::fmt::Write;
use std::time::SystemTime;

use git_branchless_invoke::CommandContext;
use git_branchless_opts::{Revset, SmartlogArgs};
use lib::core::config::{
    Hint, get_hint_enabled, get_hint_string, get_smartlog_default_revset, get_smartlog_reverse,
    print_hint_suppression_notice,
};
use lib::core::repo_ext::RepoExt;
use lib::core::rewrite::find_rewrite_target;
use lib::util::{ExitCode, EyreExitOr};
use tracing::instrument;

use lib::core::dag::{CommitSet, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::Pluralize;
use lib::core::node_descriptors::{
    BranchesDescriptor, CommitMessageDescriptor, CommitOidDescriptor,
    DifferentialRevisionDescriptor, ObsolescenceExplanationDescriptor, Redactor,
    RelativeTimeDescriptor,
};
use lib::git::{GitRunInfo, Repo};

pub use graph::{SmartlogGraph, make_smartlog_graph};
pub use render::{SmartlogOptions, render_graph};

use git_branchless_revset::resolve_commits;

mod graph {
    use std::collections::HashMap;

    use lib::core::gc::mark_commit_reachable;
    use tracing::instrument;

    use lib::core::dag::{CommitSet, CommitVertex, Dag};
    use lib::core::effects::{Effects, OperationType};
    use lib::core::eventlog::{EventCursor, EventReplayer};
    use lib::core::node_descriptors::NodeObject;
    use lib::git::{Commit, Time};
    use lib::git::{NonZeroOid, Repo};

    #[derive(Debug)]
    pub struct AncestorInfo {
        pub oid: NonZeroOid,
        pub distance: usize,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    pub struct ChildInfo {
        pub oid: NonZeroOid,
        pub is_merge_child: bool,
    }
    /// Node contained in the smartlog commit graph.
    #[derive(Debug)]
    pub struct Node<'repo> {
        /// The underlying commit object.
        pub object: NodeObject<'repo>,

        /// The OIDs of the parent nodes in the smartlog commit graph.
        ///
        /// This is different from inspecting `commit.parents()`, since the smartlog
        /// will hide most nodes from the commit graph, including parent nodes.
        pub parents: Vec<NonZeroOid>,

        /// The OIDs of the children nodes in the smartlog commit graph.
        pub children: Vec<ChildInfo>,

        /// Information about a non-immediate, non-main branch ancestor node in
        /// the smartlog commit graph.
        pub ancestor_info: Option<AncestorInfo>,

        /// The OIDs of any non-immediate descendant nodes in the smartlog commit graph.
        pub descendants: Vec<ChildInfo>,

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

        /// Indicates that this commit has descendants, but that none of them
        /// are included in the graph.
        ///
        /// This allows us to indicate a "false head" to the user. Otherwise,
        /// this commit would look like a normal, descendant-less head.
        pub num_omitted_descendants: usize,
    }

    /// Graph of commits that the user is working on.
    pub struct SmartlogGraph<'repo> {
        /// The nodes in the graph for use in rendering the smartlog.
        pub nodes: HashMap<NonZeroOid, Node<'repo>>,
    }

    impl<'repo> SmartlogGraph<'repo> {
        /// Get a list of commits stored in the graph.
        /// Returns commits in descending commit time order.
        pub fn get_commits(&self) -> Vec<Commit<'repo>> {
            let mut commits = self
                .nodes
                .values()
                .filter_map(|node| match &node.object {
                    NodeObject::Commit { commit } => Some(commit.clone()),
                    NodeObject::GarbageCollected { oid: _ } => None,
                })
                .collect::<Vec<Commit<'repo>>>();
            commits.sort_by_key(|commit| (commit.get_committer().get_time(), commit.get_oid()));
            commits.reverse();
            commits
        }
    }

    impl std::fmt::Debug for SmartlogGraph<'_> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "<CommitGraph len={}>", self.nodes.len())
        }
    }

    /// Build the smartlog graph by finding additional commits that should be displayed.
    ///
    /// For example, if you check out a commit that has intermediate parent commits
    /// between it and the main branch, those intermediate commits should be shown
    /// (or else you won't get a good idea of the line of development that happened
    /// for this commit since the main branch).
    #[instrument]
    fn build_graph<'repo>(
        effects: &Effects,
        repo: &'repo Repo,
        dag: &Dag,
        commits: &CommitSet,
    ) -> eyre::Result<SmartlogGraph<'repo>> {
        let commits_include_main =
            !dag.set_is_empty(&dag.main_branch_commit.intersection(commits))?;
        let mut graph: HashMap<NonZeroOid, Node> = {
            let mut result = HashMap::new();
            for vertex in dag.commit_set_to_vec(commits)? {
                let vertex = CommitSet::from(vertex);
                let merge_bases = if commits_include_main {
                    dag.query_gca_all(dag.main_branch_commit.union(&vertex))?
                } else {
                    dag.query_gca_all(commits.union(&vertex))?
                };
                let vertices = vertex.union(&merge_bases);

                for oid in dag.commit_set_to_vec(&vertices)? {
                    let object = match repo.find_commit(oid)? {
                        Some(commit) => NodeObject::Commit { commit },
                        None => {
                            // Assume that this commit was garbage collected.
                            NodeObject::GarbageCollected { oid }
                        }
                    };

                    result.insert(
                        oid,
                        Node {
                            object,
                            parents: Vec::new(),  // populated below
                            children: Vec::new(), // populated below
                            ancestor_info: None,
                            descendants: Vec::new(), // populated below
                            is_main: dag.is_public_commit(oid)?,
                            is_obsolete: dag.set_contains(&dag.query_obsolete_commits(), oid)?,
                            num_omitted_descendants: 0, // populated below
                        },
                    );
                }
            }
            result
        };

        let mut immediate_links: Vec<(NonZeroOid, NonZeroOid, bool)> = Vec::new();
        let mut non_immediate_links: Vec<(NonZeroOid, NonZeroOid, bool)> = Vec::new();

        let non_main_node_oids = graph
            .iter()
            .filter_map(|(child_oid, node)| if !node.is_main { Some(child_oid) } else { None });

        let graph_vertices: CommitSet = graph.keys().cloned().collect();
        for child_oid in non_main_node_oids {
            let parent_vertices = dag.query_parent_names(CommitVertex::from(*child_oid))?;

            // Find immediate parent-child links.
            match parent_vertices.as_slice() {
                [] => {}
                [first_parent_vertex, merge_parent_vertices @ ..] => {
                    if dag.set_contains(&graph_vertices, first_parent_vertex.clone())? {
                        let first_parent_oid = NonZeroOid::try_from(first_parent_vertex.clone())?;
                        immediate_links.push((*child_oid, first_parent_oid, false));
                    }
                    for merge_parent_vertex in merge_parent_vertices {
                        if dag.set_contains(&graph_vertices, merge_parent_vertex.clone())? {
                            let merge_parent_oid =
                                NonZeroOid::try_from(merge_parent_vertex.clone())?;
                            immediate_links.push((*child_oid, merge_parent_oid, true));
                        }
                    }
                }
            }

            // Find non-immediate ancestor links.
            for excluded_parent_vertex in parent_vertices {
                if dag.set_contains(&graph_vertices, excluded_parent_vertex.clone())? {
                    continue;
                }

                // Find the nearest ancestor that is included in the graph and
                // also on the same branch.

                let parent_set = CommitSet::from(excluded_parent_vertex);
                let merge_base = dag.query_gca_one(dag.main_branch_commit.union(&parent_set))?;

                let path_to_main_branch = match merge_base {
                    Some(merge_base) => dag.query_range(CommitSet::from(merge_base), parent_set)?,
                    None => CommitSet::empty(),
                };
                let nearest_branch_ancestor =
                    dag.query_heads_ancestors(path_to_main_branch.intersection(&graph_vertices))?;

                let ancestor_oids = dag.commit_set_to_vec(&nearest_branch_ancestor)?;
                for ancestor_oid in ancestor_oids.iter() {
                    non_immediate_links.push((*ancestor_oid, *child_oid, false));
                }
            }
        }

        for (child_oid, parent_oid, is_merge_link) in immediate_links.iter() {
            graph.get_mut(child_oid).unwrap().parents.push(*parent_oid);
            graph.get_mut(parent_oid).unwrap().children.push(ChildInfo {
                oid: *child_oid,
                is_merge_child: *is_merge_link,
            });
        }

        for (ancestor_oid, descendent_oid, is_merge_link) in non_immediate_links.iter() {
            let distance = dag.set_count(
                &dag.query_range(
                    CommitSet::from(*ancestor_oid),
                    CommitSet::from(*descendent_oid),
                )?
                .difference(&vec![*ancestor_oid, *descendent_oid].into_iter().collect()),
            )?;
            graph.get_mut(descendent_oid).unwrap().ancestor_info = Some(AncestorInfo {
                oid: *ancestor_oid,
                distance,
            });
            graph
                .get_mut(ancestor_oid)
                .unwrap()
                .descendants
                .push(ChildInfo {
                    oid: *descendent_oid,
                    is_merge_child: *is_merge_link,
                })
        }

        for (oid, node) in graph.iter_mut() {
            let oid_set = CommitSet::from(*oid);
            let is_main_head = !dag.set_is_empty(&dag.main_branch_commit.intersection(&oid_set))?;
            let ancestor_of_main = node.is_main && !is_main_head;
            let has_descendants_in_graph =
                !node.children.is_empty() || !node.descendants.is_empty();

            if ancestor_of_main || has_descendants_in_graph {
                continue;
            }

            // This node has no descendants in the graph, so it's a
            // false head if it has *any* visible descendants.
            let descendants_not_in_graph =
                dag.query_descendants(oid_set.clone())?.difference(&oid_set);
            let descendants_not_in_graph = dag.filter_visible_commits(descendants_not_in_graph)?;

            node.num_omitted_descendants = dag.set_count(&descendants_not_in_graph)?;
        }

        Ok(SmartlogGraph { nodes: graph })
    }

    /// Sort children nodes of the commit graph in a standard order, for determinism
    /// in output.
    fn sort_children(graph: &mut SmartlogGraph) {
        let commit_times: HashMap<NonZeroOid, Option<Time>> = graph
            .nodes
            .iter()
            .map(|(oid, node)| {
                (
                    *oid,
                    match &node.object {
                        NodeObject::Commit { commit } => Some(commit.get_time()),
                        NodeObject::GarbageCollected { oid: _ } => None,
                    },
                )
            })
            .collect();
        for node in graph.nodes.values_mut() {
            node.children.sort_by_key(
                |ChildInfo {
                     oid,
                     is_merge_child,
                 }| (&commit_times[oid], *is_merge_child, oid.to_string()),
            );
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
        commits: &CommitSet,
        exact: bool,
    ) -> eyre::Result<SmartlogGraph<'repo>> {
        let (effects, _progress) = effects.start_operation(OperationType::MakeGraph);

        let mut graph = {
            let (effects, _progress) = effects.start_operation(OperationType::WalkCommits);

            // HEAD and main head are automatically included unless `exact` is set
            let commits = if exact {
                commits.clone()
            } else {
                commits
                    .union(&dag.head_commit)
                    .union(&dag.main_branch_commit)
            };

            for oid in dag.commit_set_to_vec(&commits)? {
                mark_commit_reachable(repo, oid)?;
            }

            build_graph(&effects, repo, dag, &commits)?
        };
        sort_children(&mut graph);
        Ok(graph)
    }
}

mod render {
    use std::cmp::Ordering;
    use std::collections::HashSet;

    use cursive_core::theme::{BaseColor, Effect};
    use cursive_core::utils::markup::StyledString;
    use tracing::instrument;

    use lib::core::dag::{CommitSet, Dag};
    use lib::core::effects::Effects;
    use lib::core::formatting::{Glyphs, StyledStringBuilder};
    use lib::core::formatting::{Pluralize, set_effect};
    use lib::core::node_descriptors::{NodeDescriptor, render_node_descriptors};
    use lib::git::{NonZeroOid, Repo};

    use git_branchless_opts::{ResolveRevsetOptions, Revset};

    use super::graph::{AncestorInfo, ChildInfo, SmartlogGraph};

    /// Split fully-independent subgraphs into multiple graphs.
    ///
    /// This is intended to handle the situation of having multiple lines of work
    /// rooted from different commits in the main branch.
    ///
    /// Returns the list such that the topologically-earlier subgraphs are first in
    /// the list (i.e. those that would be rendered at the bottom of the smartlog).
    fn split_commit_graph_by_roots(
        repo: &Repo,
        dag: &Dag,
        graph: &SmartlogGraph,
    ) -> Vec<NonZeroOid> {
        let mut root_commit_oids: Vec<NonZeroOid> = graph
            .nodes
            .iter()
            .filter(|(_oid, node)| node.parents.is_empty() && node.ancestor_info.is_none())
            .map(|(oid, _node)| oid)
            .copied()
            .collect();

        let compare = |lhs_oid: &NonZeroOid, rhs_oid: &NonZeroOid| -> Ordering {
            let lhs_commit = repo.find_commit(*lhs_oid);
            let rhs_commit = repo.find_commit(*rhs_oid);

            let (lhs_commit, rhs_commit) = match (lhs_commit, rhs_commit) {
                (Ok(Some(lhs_commit)), Ok(Some(rhs_commit))) => (lhs_commit, rhs_commit),
                _ => return lhs_oid.cmp(rhs_oid),
            };

            let merge_base_oid =
                dag.query_gca_one(vec![*lhs_oid, *rhs_oid].into_iter().collect::<CommitSet>());
            let merge_base_oid = match merge_base_oid {
                Err(_) => return lhs_oid.cmp(rhs_oid),
                Ok(None) => None,
                Ok(Some(merge_base_oid)) => NonZeroOid::try_from(merge_base_oid).ok(),
            };

            match merge_base_oid {
                // lhs was topologically first, so it should be sorted earlier in the list.
                Some(merge_base_oid) if merge_base_oid == *lhs_oid => Ordering::Less,
                Some(merge_base_oid) if merge_base_oid == *rhs_oid => Ordering::Greater,

                // The commits were not orderable (pathlogical situation). Let's
                // just order them by timestamp in that case to produce a consistent
                // and reasonable guess at the intended topological ordering.
                Some(_) | None => match lhs_commit.get_time().cmp(&rhs_commit.get_time()) {
                    result @ Ordering::Less | result @ Ordering::Greater => result,
                    Ordering::Equal => lhs_oid.cmp(rhs_oid),
                },
            }
        };

        root_commit_oids.sort_by(compare);
        root_commit_oids
    }

    #[instrument(skip(commit_descriptors, graph))]
    fn get_child_output(
        glyphs: &Glyphs,
        graph: &SmartlogGraph,
        root_oids: &[NonZeroOid],
        commit_descriptors: &mut [&mut dyn NodeDescriptor],
        head_oid: Option<NonZeroOid>,
        current_oid: NonZeroOid,
        last_child_line_char: Option<&str>,
    ) -> eyre::Result<Vec<StyledString>> {
        let current_node = &graph.nodes[&current_oid];
        let is_head = Some(current_oid) == head_oid;

        let mut lines = vec![];

        if let Some(AncestorInfo { oid: _, distance }) = current_node.ancestor_info {
            lines.push(
                StyledStringBuilder::new()
                    .append_plain(glyphs.commit_omitted)
                    .append_plain(" ")
                    .append_styled(
                        Pluralize {
                            determiner: None,
                            amount: distance,
                            unit: ("omitted commit", "omitted commits"),
                        }
                        .to_string(),
                        Effect::Dim,
                    )
                    .build(),
            );
            lines.push(StyledString::plain(glyphs.vertical_ellipsis));
        };

        if let [_, merge_parents @ ..] = current_node.parents.as_slice() {
            if !merge_parents.is_empty() {
                for merge_parent_oid in merge_parents {
                    let merge_parent_node = &graph.nodes[merge_parent_oid];
                    lines.push(
                        StyledStringBuilder::new()
                            .append_plain(last_child_line_char.unwrap_or(glyphs.line))
                            .append_plain(" ")
                            .append_styled(
                                format!("{} (merge) ", glyphs.commit_merge),
                                BaseColor::Blue.dark(),
                            )
                            .append(render_node_descriptors(
                                glyphs,
                                &merge_parent_node.object,
                                commit_descriptors,
                            )?)
                            .build(),
                    );
                }
                lines.push(StyledString::plain(format!(
                    "{}{}",
                    glyphs.line_with_offshoot, glyphs.merge,
                )));
            }
        }

        lines.push({
            let cursor = match (current_node.is_main, current_node.is_obsolete, is_head) {
                (false, false, false) => glyphs.commit_visible,
                (false, false, true) => glyphs.commit_visible_head,
                (false, true, false) => glyphs.commit_obsolete,
                (false, true, true) => glyphs.commit_obsolete_head,
                (true, false, false) => glyphs.commit_main,
                (true, false, true) => glyphs.commit_main_head,
                (true, true, false) => glyphs.commit_main_obsolete,
                (true, true, true) => glyphs.commit_main_obsolete_head,
            };
            let text = render_node_descriptors(glyphs, &current_node.object, commit_descriptors)?;
            let first_line = StyledStringBuilder::new()
                .append_plain(cursor)
                .append_plain(" ")
                .append(text)
                .build();
            if is_head {
                set_effect(first_line, Effect::Bold)
            } else {
                first_line
            }
        });

        if current_node.num_omitted_descendants > 0 {
            lines.push(StyledString::plain(glyphs.vertical_ellipsis));
            lines.push(
                StyledStringBuilder::new()
                    .append_plain(glyphs.commit_omitted)
                    .append_plain(" ")
                    .append_styled(
                        Pluralize {
                            determiner: None,
                            amount: current_node.num_omitted_descendants,
                            unit: ("omitted descendant commit", "omitted descendant commits"),
                        }
                        .to_string(),
                        Effect::Dim,
                    )
                    .build(),
            );
        };

        let children: Vec<ChildInfo> = current_node
            .children
            .iter()
            .filter(
                |ChildInfo {
                     oid,
                     is_merge_child: _,
                 }| graph.nodes.contains_key(oid),
            )
            .cloned()
            .collect();
        let descendants: HashSet<ChildInfo> = current_node
            .descendants
            .iter()
            .filter(
                |ChildInfo {
                     oid,
                     is_merge_child: _,
                 }| graph.nodes.contains_key(oid),
            )
            .cloned()
            .collect();
        for (child_idx, child_info) in children.iter().chain(descendants.iter()).enumerate() {
            let ChildInfo {
                oid: child_oid,
                is_merge_child,
            } = child_info;
            if root_oids.contains(child_oid) {
                // Will be rendered by the parent.
                continue;
            }
            if *is_merge_child {
                // lines.push(StyledString::plain(format!(
                //     "{}{}",
                //     glyphs.line_with_offshoot, glyphs.split
                // )));
                lines.push(
                    StyledStringBuilder::new()
                        // .append_plain(last_child_line_char.unwrap_or(glyphs.line))
                        // .append_plain(" ")
                        .append_styled(
                            format!("{} (merge) ", glyphs.commit_merge),
                            BaseColor::Blue.dark(),
                        )
                        .append(render_node_descriptors(
                            glyphs,
                            &graph.nodes[child_oid].object,
                            commit_descriptors,
                        )?)
                        .build(),
                );
                continue;
            }

            let is_last_child = child_idx == (children.len() + descendants.len()) - 1;
            lines.push(StyledString::plain(
                if !is_last_child || last_child_line_char.is_some() {
                    format!("{}{}", glyphs.line_with_offshoot, glyphs.split)
                } else if current_node.descendants.is_empty() {
                    glyphs.line.to_string()
                } else {
                    glyphs.vertical_ellipsis.to_string()
                },
            ));

            let child_output = get_child_output(
                glyphs,
                graph,
                root_oids,
                commit_descriptors,
                head_oid,
                *child_oid,
                None,
            )?;
            for child_line in child_output {
                let line = if is_last_child {
                    match last_child_line_char {
                        Some(last_child_line_char) => StyledStringBuilder::new()
                            .append_plain(format!("{last_child_line_char} "))
                            .append(child_line)
                            .build(),
                        None => child_line,
                    }
                } else {
                    StyledStringBuilder::new()
                        .append_plain(format!("{} ", glyphs.line))
                        .append(child_line)
                        .build()
                };
                lines.push(line)
            }
        }
        Ok(lines)
    }

    /// Render a pretty graph starting from the given root OIDs in the given graph.
    #[instrument(skip(commit_descriptors, graph))]
    fn get_output(
        glyphs: &Glyphs,
        dag: &Dag,
        graph: &SmartlogGraph,
        commit_descriptors: &mut [&mut dyn NodeDescriptor],
        head_oid: Option<NonZeroOid>,
        root_oids: &[NonZeroOid],
    ) -> eyre::Result<Vec<StyledString>> {
        let mut lines = Vec::new();

        // Determine if the provided OID has the provided parent OID as a parent.
        //
        // This returns `true` in strictly more cases than checking `graph`,
        // since there may be links between adjacent main branch commits which
        // are not reflected in `graph`.
        let has_real_parent = |oid: NonZeroOid, parent_oid: NonZeroOid| -> eyre::Result<bool> {
            let parents = dag.query_parents(CommitSet::from(oid))?;
            let result = dag.set_contains(&parents, parent_oid)?;
            Ok(result)
        };

        for (root_idx, root_oid) in root_oids.iter().enumerate() {
            if !dag.set_is_empty(&dag.query_parents(CommitSet::from(*root_oid))?)? {
                let line = if root_idx > 0 && has_real_parent(*root_oid, root_oids[root_idx - 1])? {
                    StyledString::plain(glyphs.line.to_owned())
                } else {
                    StyledString::plain(glyphs.vertical_ellipsis.to_owned())
                };
                lines.push(line);
            } else if root_idx > 0 {
                // Pathological case: multiple topologically-unrelated roots.
                // Separate them with a newline.
                lines.push(StyledString::new());
            }

            let last_child_line_char = {
                if root_idx == root_oids.len() - 1 {
                    None
                } else if has_real_parent(root_oids[root_idx + 1], *root_oid)? {
                    Some(glyphs.line)
                } else {
                    Some(glyphs.vertical_ellipsis)
                }
            };

            let child_output = get_child_output(
                glyphs,
                graph,
                root_oids,
                commit_descriptors,
                head_oid,
                *root_oid,
                last_child_line_char,
            )?;
            lines.extend(child_output.into_iter());
        }

        Ok(lines)
    }

    /// Render the smartlog graph and write it to the provided stream.
    #[instrument(skip(commit_descriptors, graph))]
    pub fn render_graph(
        effects: &Effects,
        repo: &Repo,
        dag: &Dag,
        graph: &SmartlogGraph,
        head_oid: Option<NonZeroOid>,
        commit_descriptors: &mut [&mut dyn NodeDescriptor],
    ) -> eyre::Result<Vec<StyledString>> {
        let root_oids = split_commit_graph_by_roots(repo, dag, graph);
        let lines = get_output(
            effects.get_glyphs(),
            dag,
            graph,
            commit_descriptors,
            head_oid,
            &root_oids,
        )?;
        Ok(lines)
    }

    /// Options for rendering the smartlog.
    #[derive(Debug, Default)]
    pub struct SmartlogOptions {
        /// The point in time at which to show the smartlog. If not provided,
        /// renders the smartlog as of the current time. If negative, is treated
        /// as an offset from the current event.
        pub event_id: Option<isize>,

        /// The commits to render. These commits, plus any related commits, will
        /// be rendered. If not provided, the user's default revset will be used
        /// instead.
        pub revset: Option<Revset>,

        /// The options to use when resolving the revset.
        pub resolve_revset_options: ResolveRevsetOptions,

        /// Deprecated
        /// Reverse the ordering of items in the smartlog output, list the most
        /// recent commits first.
        pub reverse: bool,

        /// Normally HEAD and the main branch are included. Set this to exclude them.
        pub exact: bool,
    }
}

/// Display a nice graph of commits you've recently worked on.
#[instrument]
pub fn smartlog(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    options: SmartlogOptions,
) -> EyreExitOr<()> {
    let SmartlogOptions {
        event_id,
        revset,
        resolve_revset_options,
        reverse,
        exact,
    } = options;

    let repo = Repo::from_dir(&git_run_info.working_directory)?;
    let head_info = repo.get_head_info()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let (references_snapshot, event_cursor) = {
        let default_cursor = event_replayer.make_default_cursor();
        match event_id {
            None => (repo.get_references_snapshot()?, default_cursor),
            Some(event_id) => {
                let event_cursor = match event_id.cmp(&0) {
                    Ordering::Less => event_replayer.advance_cursor(default_cursor, event_id),
                    Ordering::Equal | Ordering::Greater => event_replayer.make_cursor(event_id),
                };
                let references_snapshot =
                    event_replayer.get_references_snapshot(&repo, event_cursor)?;
                (references_snapshot, event_cursor)
            }
        }
    };
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let revset = match revset {
        Some(revset) => revset,
        None => Revset(get_smartlog_default_revset(&repo)?),
    };
    let commits =
        match resolve_commits(effects, &repo, &mut dag, &[revset], &resolve_revset_options) {
            Ok(result) => match result.as_slice() {
                [commit_set] => commit_set.clone(),
                other => panic!("Expected exactly 1 result from resolve commits, got: {other:?}"),
            },
            Err(err) => {
                err.describe(effects)?;
                return Ok(Err(ExitCode(1)));
            }
        };

    let graph = make_smartlog_graph(
        effects,
        &repo,
        &dag,
        &event_replayer,
        event_cursor,
        &commits,
        exact,
    )?;

    let reverse = if reverse {
        writeln!(
            effects.get_error_stream(),
            "WARNING: The `--reverse` flag is deprecated.\nPlease use the `branchless.smartlog.reverse` configuration option."
        )?;
        true
    } else {
        get_smartlog_reverse(&repo)?
    };

    let mut lines = render_graph(
        &effects.reverse_order(reverse),
        &repo,
        &dag,
        &graph,
        references_snapshot.head_oid,
        &mut [
            &mut CommitOidDescriptor::new(true)?,
            &mut RelativeTimeDescriptor::new(&repo, SystemTime::now())?,
            &mut ObsolescenceExplanationDescriptor::new(
                &event_replayer,
                event_replayer.make_default_cursor(),
            )?,
            &mut BranchesDescriptor::new(
                &repo,
                &head_info,
                &references_snapshot,
                &Redactor::Disabled,
            )?,
            &mut DifferentialRevisionDescriptor::new(&repo, &Redactor::Disabled)?,
            &mut CommitMessageDescriptor::new(&Redactor::Disabled)?,
        ],
    )?
    .into_iter();
    while let Some(line) = if reverse {
        lines.next_back()
    } else {
        lines.next()
    } {
        writeln!(
            effects.get_output_stream(),
            "{}",
            effects.get_glyphs().render(line)?
        )?;
    }

    if !resolve_revset_options.show_hidden_commits
        && get_hint_enabled(&repo, Hint::SmartlogFixAbandoned)?
    {
        let commits_with_abandoned_children: CommitSet = graph
            .nodes
            .iter()
            .filter_map(|(oid, node)| {
                if node.is_obsolete
                    && find_rewrite_target(&event_replayer, event_cursor, *oid).is_some()
                {
                    Some(*oid)
                } else {
                    None
                }
            })
            .collect();
        let children = dag.query_children(commits_with_abandoned_children)?;
        let num_abandoned_children =
            dag.set_count(&children.difference(&dag.query_obsolete_commits()))?;
        if num_abandoned_children > 0 {
            writeln!(
                effects.get_output_stream(),
                "{}: there {} in your commit graph",
                effects.get_glyphs().render(get_hint_string())?,
                Pluralize {
                    determiner: Some(("is", "are")),
                    amount: num_abandoned_children,
                    unit: ("abandoned commit", "abandoned commits"),
                },
            )?;
            writeln!(
                effects.get_output_stream(),
                "{}: to fix this, run: git restack",
                effects.get_glyphs().render(get_hint_string())?,
            )?;
            print_hint_suppression_notice(effects, Hint::SmartlogFixAbandoned)?;
        }
    }

    Ok(Ok(()))
}

/// `smartlog` command.
#[instrument]
pub fn command_main(ctx: CommandContext, args: SmartlogArgs) -> EyreExitOr<()> {
    let CommandContext {
        effects,
        git_run_info,
    } = ctx;
    let SmartlogArgs {
        event_id,
        revset,
        resolve_revset_options,
        reverse,
        exact,
    } = args;

    smartlog(
        &effects,
        &git_run_info,
        SmartlogOptions {
            event_id,
            revset,
            resolve_revset_options,
            reverse,
            exact,
        },
    )
}
