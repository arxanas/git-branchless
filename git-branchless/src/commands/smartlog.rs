//! Display a graph of commits that the user has worked on recently.
//!
//! The set of commits that are still being worked on is inferred from the event
//! log; see the `eventlog` module.

use std::cmp::Ordering;
use std::fmt::Write;
use std::time::SystemTime;

use eden_dag::DagAlgorithm;
use lib::core::config::{get_hint_enabled, get_hint_string, print_hint_suppression_notice, Hint};
use lib::core::repo_ext::RepoExt;
use lib::core::rewrite::find_rewrite_target;
use lib::util::ExitCode;
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

pub use graph::{make_smartlog_graph, SmartlogGraph};
pub use render::{render_graph, SmartlogOptions};

use crate::revset::resolve_commits;

mod graph {
    use std::collections::HashMap;

    use eden_dag::DagAlgorithm;
    use lib::core::gc::mark_commit_reachable;
    use tracing::instrument;

    use lib::core::dag::{commit_set_to_vec, CommitSet, Dag};
    use lib::core::effects::{Effects, OperationType};
    use lib::core::eventlog::{EventCursor, EventReplayer};
    use lib::core::node_descriptors::NodeObject;
    use lib::git::{Commit, Time};
    use lib::git::{NonZeroOid, Repo};

    /// Node contained in the smartlog commit graph.
    #[derive(Debug)]
    pub struct Node<'repo> {
        /// The underlying commit object.
        pub object: NodeObject<'repo>,

        /// The OID of the parent node in the smartlog commit graph.
        ///
        /// This is different from inspecting `commit.parents()`, since the smartlog
        /// will hide most nodes from the commit graph, including parent nodes.
        pub parent: Option<NonZeroOid>,

        /// The OIDs of the children nodes in the smartlog commit graph.
        pub children: Vec<NonZeroOid>,

        /// Does this commit have any non-immediate, non-main branch ancestor
        /// nodes in the smartlog commit graph?
        pub has_ancestors: bool,

        /// The OIDs of any non-immediate descendant nodes in the smartlog commit graph.
        pub descendants: Vec<NonZeroOid>,

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
        /// This allows us to indicate this "false head" to the user. Otherwise,
        /// this commit would look like a normal, descendant-less head.
        pub is_false_head: bool,
    }

    /// Graph of commits that the user is working on.
    pub struct SmartlogGraph<'repo> {
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
        let mut graph: HashMap<NonZeroOid, Node> = {
            let mut result = HashMap::new();
            for vertex in commit_set_to_vec(commits)? {
                let vertex = CommitSet::from(vertex);
                let merge_bases = dag.query().gca_all(dag.main_branch_commit.union(&vertex))?;
                let vertices = vertex.union(&merge_bases);

                for oid in commit_set_to_vec(&vertices)? {
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
                            parent: None,         // populated below
                            children: Vec::new(), // populated below
                            has_ancestors: false,
                            descendants: Vec::new(), // populated below
                            is_main: dag.is_public_commit(oid)?,
                            is_obsolete: dag.query_obsolete_commits().contains(&oid.into())?,
                            is_false_head: false,
                        },
                    );
                }
            }
            result
        };

        let mut immediate_links: Vec<(NonZeroOid, NonZeroOid)> = Vec::new();
        let mut non_immediate_links: Vec<(NonZeroOid, NonZeroOid)> = Vec::new();

        let non_main_node_oids =
            graph
                .iter()
                .filter_map(|(child_oid, node)| if !node.is_main { Some(child_oid) } else { None });

        let graph_vertices: CommitSet = graph.keys().cloned().collect();
        for child_oid in non_main_node_oids {
            let parent_vertices = dag.query().parents(CommitSet::from(*child_oid))?;

            // Find immediate parent-child links.
            let parents_in_graph = parent_vertices.intersection(&graph_vertices);
            let parent_oids = commit_set_to_vec(&parents_in_graph)?;
            for parent_oid in parent_oids {
                immediate_links.push((*child_oid, parent_oid))
            }

            if parent_vertices.count()? != parents_in_graph.count()? {
                // Find non-immediate ancestor links.
                let excluded_parents = parent_vertices.difference(&graph_vertices);
                let excluded_parent_oids = commit_set_to_vec(&excluded_parents)?;
                for parent_oid in excluded_parent_oids {
                    // Find the nearest ancestor that is included in the graph and
                    // also on the same branch.

                    let parent_set = CommitSet::from(parent_oid);
                    let merge_base = dag
                        .query()
                        .gca_one(dag.main_branch_commit.union(&parent_set))?;

                    let path_to_main_branch = match merge_base {
                        Some(merge_base) => {
                            dag.query().range(CommitSet::from(merge_base), parent_set)?
                        }
                        None => CommitSet::empty(),
                    };
                    let nearest_branch_ancestor = dag
                        .query()
                        .heads_ancestors(path_to_main_branch.intersection(&graph_vertices))?;

                    let ancestor_oids = commit_set_to_vec(&nearest_branch_ancestor)?;
                    for ancestor_oid in ancestor_oids.iter() {
                        non_immediate_links.push((*ancestor_oid, *child_oid));
                    }
                }
            }
        }

        for (child_oid, parent_oid) in immediate_links.iter() {
            graph.get_mut(child_oid).unwrap().parent = Some(*parent_oid);
            graph.get_mut(parent_oid).unwrap().children.push(*child_oid);
        }

        for (ancestor_oid, descendent_oid) in non_immediate_links.iter() {
            graph.get_mut(descendent_oid).unwrap().has_ancestors = true;
            graph
                .get_mut(ancestor_oid)
                .unwrap()
                .descendants
                .push(*descendent_oid);
        }

        for (oid, node) in graph.iter_mut() {
            let oid_set = CommitSet::from(*oid);
            let is_main_head = !dag.main_branch_commit.intersection(&oid_set).is_empty()?;
            let ancestor_of_main = node.is_main && !is_main_head;
            let has_descendants_in_graph =
                !node.children.is_empty() || !node.descendants.is_empty();

            if ancestor_of_main || has_descendants_in_graph {
                continue;
            }

            // This node has no descendants in the graph, so it's a
            // false head if it has *any* (non-obsolete) children.
            let children_not_in_graph = dag
                .query()
                .children(oid_set)?
                .difference(&dag.query_obsolete_commits());

            node.is_false_head = !children_not_in_graph.is_empty()?;
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
            node.children
                .sort_by_key(|child_oid| (&commit_times[child_oid], child_oid.to_string()));
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
    ) -> eyre::Result<SmartlogGraph<'repo>> {
        let (effects, _progress) = effects.start_operation(OperationType::MakeGraph);

        let mut graph = {
            let (effects, _progress) = effects.start_operation(OperationType::WalkCommits);

            // HEAD and main head must be included
            let commits = commits
                .union(&dag.head_commit)
                .union(&dag.main_branch_commit);

            for oid in commit_set_to_vec(&commits)? {
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
    use std::convert::TryFrom;

    use cursive::theme::Effect;
    use cursive::utils::markup::StyledString;
    use eden_dag::DagAlgorithm;
    use tracing::instrument;

    use lib::core::dag::{CommitSet, CommitVertex, Dag};
    use lib::core::effects::Effects;
    use lib::core::formatting::set_effect;
    use lib::core::formatting::{Glyphs, StyledStringBuilder};
    use lib::core::node_descriptors::{render_node_descriptors, NodeDescriptor};
    use lib::git::{NonZeroOid, Repo};

    use crate::opts::{ResolveRevsetOptions, Revset};

    use super::graph::SmartlogGraph;

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
            .filter(|(_oid, node)| {
                // Common case: on main w/ no parents in graph, eg a merge base
                node.parent.is_none() && node.is_main ||
                    // Pathological cases: orphaned, garbage collected, etc
                    node.parent.is_none()
                        && !node.is_main
                        && node.children.is_empty()
                        && node.descendants.is_empty()
                        && !node.has_ancestors
            })
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

            let merge_base_oid = dag
                .query()
                .gca_one(vec![*lhs_oid, *rhs_oid].into_iter().collect::<CommitSet>());
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

        let text = render_node_descriptors(glyphs, &current_node.object, commit_descriptors)?;
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

        let mut lines = vec![];

        if current_node.has_ancestors {
            lines.push(StyledString::plain(glyphs.vertical_ellipsis.to_string()));
        };

        lines.push({
            let mut first_line = StyledString::new();
            first_line.append_plain(cursor);
            first_line.append_plain(" ");
            first_line.append(text);
            if is_head {
                set_effect(first_line, Effect::Bold)
            } else {
                first_line
            }
        });

        if current_node.is_false_head {
            lines.push(StyledString::plain(glyphs.vertical_ellipsis.to_string()));
        };

        let children: Vec<_> = current_node
            .children
            .iter()
            .filter(|child_oid| graph.nodes.contains_key(child_oid))
            .copied()
            .collect();
        let descendants: HashSet<_> = current_node
            .descendants
            .iter()
            .filter(|descendent_oid| graph.nodes.contains_key(descendent_oid))
            .copied()
            .collect();
        for (child_idx, child_oid) in children.iter().chain(descendants.iter()).enumerate() {
            if root_oids.contains(child_oid) {
                // Will be rendered by the parent.
                continue;
            }

            let is_last_child = child_idx == (children.len() + descendants.len()) - 1;
            if is_last_child {
                let line = match last_child_line_char {
                    Some(_) => Some(StyledString::plain(format!(
                        "{}{}",
                        glyphs.line_with_offshoot, glyphs.slash
                    ))),
                    None if current_node.descendants.is_empty() => {
                        Some(StyledString::plain(glyphs.line.to_string()))
                    }
                    None => None,
                };
                if let Some(line) = line {
                    lines.push(line);
                }
            } else {
                lines.push(StyledString::plain(format!(
                    "{}{}",
                    glyphs.line_with_offshoot, glyphs.slash
                )));
            }

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
                            .append_plain(format!("{} ", last_child_line_char))
                            .append(child_line)
                            .build(),
                        None => child_line,
                    }
                } else {
                    StyledStringBuilder::new()
                        .append_plain(format!(
                            "{} ",
                            if !current_node.descendants.is_empty() {
                                glyphs.vertical_ellipsis
                            } else {
                                glyphs.line
                            }
                        ))
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
            let parents = dag.query().parents(CommitSet::from(oid))?;
            let result = parents.contains(&CommitVertex::from(parent_oid))?;
            Ok(result)
        };

        for (root_idx, root_oid) in root_oids.iter().enumerate() {
            if !dag
                .query()
                .parents(CommitSet::from(*root_oid))?
                .is_empty()?
            {
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
    #[derive(Debug)]
    pub struct SmartlogOptions {
        /// The point in time at which to show the smartlog. If not provided,
        /// renders the smartlog as of the current time. If negative, is treated
        /// as an offset from the current event.
        pub event_id: Option<isize>,

        /// The commits to render. These commits, plus any related commits, will
        /// be rendered.
        pub revset: Revset,

        pub resolve_revset_options: ResolveRevsetOptions,
    }

    impl Default for SmartlogOptions {
        fn default() -> Self {
            Self {
                event_id: Default::default(),
                revset: Revset(
                    "((draft() | branches() | @) % main()) | branches() | @".to_string(),
                ),
                resolve_revset_options: Default::default(),
            }
        }
    }
}

/// Display a nice graph of commits you've recently worked on.
#[instrument]
pub fn smartlog(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    options: &SmartlogOptions,
) -> eyre::Result<ExitCode> {
    let SmartlogOptions {
        event_id,
        revset,
        resolve_revset_options,
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
                    Ordering::Less => event_replayer.advance_cursor(default_cursor, *event_id),
                    Ordering::Equal | Ordering::Greater => event_replayer.make_cursor(*event_id),
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

    let commits = match resolve_commits(
        effects,
        &repo,
        &mut dag,
        &[revset.clone()],
        resolve_revset_options,
    ) {
        Ok(result) => match result.as_slice() {
            [commit_set] => commit_set.clone(),
            other => panic!(
                "Expected exactly 1 result from resolve commits, got: {:?}",
                other
            ),
        },
        Err(err) => {
            err.describe(effects)?;
            return Ok(ExitCode(1));
        }
    };

    let graph = make_smartlog_graph(
        effects,
        &repo,
        &dag,
        &event_replayer,
        event_cursor,
        &commits,
    )?;

    let lines = render_graph(
        effects,
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
    )?;
    for line in lines {
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
        let children = dag.query().children(commits_with_abandoned_children)?;
        let num_abandoned_children = children.difference(&dag.query_obsolete_commits()).count()?;
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

    Ok(ExitCode(0))
}
