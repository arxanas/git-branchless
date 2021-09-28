//! Display a graph of commits that the user has worked on recently.
//!
//! The set of commits that are still being worked on is inferred from the event
//! log; see the `eventlog` module.

use std::cmp::Ordering;
use std::fmt::Write;
use std::time::SystemTime;

use cursive::theme::Effect;
use cursive::utils::markup::StyledString;
use tracing::instrument;

use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::formatting::set_effect;
use crate::core::formatting::{printable_styled_string, Glyphs, StyledStringBuilder};
use crate::core::graph::{make_smartlog_graph, SmartlogGraph};
use crate::core::metadata::{
    render_commit_metadata, BranchesProvider, CommitMessageProvider, CommitMetadataProvider,
    CommitOidProvider, DifferentialRevisionProvider, ObsolescenceExplanationProvider,
    RelativeTimeProvider,
};
use crate::git::{Dag, NonZeroOid, Repo};
use crate::tui::Effects;

/// Split fully-independent subgraphs into multiple graphs.
///
/// This is intended to handle the situation of having multiple lines of work
/// rooted from different commits in the main branch.
///
/// Returns the list such that the topologically-earlier subgraphs are first in
/// the list (i.e. those that would be rendered at the bottom of the smartlog).
fn split_commit_graph_by_roots(
    effects: &Effects,
    repo: &Repo,
    dag: &Dag,
    graph: &SmartlogGraph,
) -> Vec<NonZeroOid> {
    let mut root_commit_oids: Vec<NonZeroOid> = graph
        .iter()
        .filter(|(_oid, node)| node.parent.is_none())
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

        let merge_base_oid = dag.get_one_merge_base_oid(effects, repo, *lhs_oid, *rhs_oid);
        let merge_base_oid = match merge_base_oid {
            Err(_) => return lhs_oid.cmp(rhs_oid),
            Ok(merge_base_oid) => merge_base_oid,
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

#[instrument(skip(commit_metadata_providers, graph))]
fn get_child_output(
    glyphs: &Glyphs,
    graph: &SmartlogGraph,
    root_oids: &[NonZeroOid],
    commit_metadata_providers: &mut [&mut dyn CommitMetadataProvider],
    head_oid: Option<NonZeroOid>,
    current_oid: NonZeroOid,
    last_child_line_char: Option<&str>,
) -> eyre::Result<Vec<StyledString>> {
    let current_node = &graph[&current_oid];
    let is_head = Some(current_node.commit.get_oid()) == head_oid;

    let text = render_commit_metadata(&current_node.commit, commit_metadata_providers)?;
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

    let first_line = {
        let mut first_line = StyledString::new();
        first_line.append_plain(cursor);
        first_line.append_plain(" ");
        first_line.append(text);
        if is_head {
            set_effect(first_line, Effect::Bold)
        } else {
            first_line
        }
    };

    let mut lines = vec![first_line];
    let children: Vec<_> = current_node
        .children
        .iter()
        .filter(|child_oid| graph.contains_key(child_oid))
        .copied()
        .collect();
    for (child_idx, child_oid) in children.iter().enumerate() {
        if root_oids.contains(child_oid) {
            // Will be rendered by the parent.
            continue;
        }

        if child_idx == children.len() - 1 {
            let line = match last_child_line_char {
                Some(_) => {
                    StyledString::plain(format!("{}{}", glyphs.line_with_offshoot, glyphs.slash))
                }

                None => StyledString::plain(glyphs.line.to_string()),
            };
            lines.push(line)
        } else {
            lines.push(StyledString::plain(format!(
                "{}{}",
                glyphs.line_with_offshoot, glyphs.slash
            )))
        }

        let child_output = get_child_output(
            glyphs,
            graph,
            root_oids,
            commit_metadata_providers,
            head_oid,
            *child_oid,
            None,
        )?;
        for child_line in child_output {
            let line = if child_idx == children.len() - 1 {
                match last_child_line_char {
                    Some(last_child_line_char) => StyledStringBuilder::new()
                        .append_plain(format!("{} ", last_child_line_char))
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
#[instrument(skip(commit_metadata_providers, graph))]
fn get_output(
    glyphs: &Glyphs,
    graph: &SmartlogGraph,
    commit_metadata_providers: &mut [&mut dyn CommitMetadataProvider],
    head_oid: Option<NonZeroOid>,
    root_oids: &[NonZeroOid],
) -> eyre::Result<Vec<StyledString>> {
    let mut lines = Vec::new();

    // Determine if the provided OID has the provided parent OID as a parent.
    //
    // This returns `true` in strictly more cases than checking `graph`,
    // since there may be links between adjacent main branch commits which
    // are not reflected in `graph`.
    let has_real_parent = |oid: NonZeroOid, parent_oid: NonZeroOid| -> bool {
        graph[&oid]
            .commit
            .get_parent_oids()
            .into_iter()
            .any(|parent_oid2| parent_oid2 == parent_oid)
    };

    for (root_idx, root_oid) in root_oids.iter().enumerate() {
        let root_node = &graph[root_oid];
        if root_node.commit.get_parent_count() > 0 {
            let line = if root_idx > 0 && has_real_parent(*root_oid, root_oids[root_idx - 1]) {
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
            } else {
                let next_root_oid = root_oids[root_idx + 1];
                if has_real_parent(next_root_oid, *root_oid) {
                    Some(glyphs.line)
                } else {
                    Some(glyphs.vertical_ellipsis)
                }
            }
        };

        let child_output = get_child_output(
            glyphs,
            graph,
            root_oids,
            commit_metadata_providers,
            head_oid,
            *root_oid,
            last_child_line_char,
        )?;
        lines.extend(child_output.into_iter());
    }

    Ok(lines)
}

/// Render the smartlog graph and write it to the provided stream.
#[instrument(skip(commit_metadata_providers, graph))]
pub fn render_graph(
    effects: &Effects,
    repo: &Repo,
    dag: &Dag,
    graph: &SmartlogGraph,
    head_oid: Option<NonZeroOid>,
    commit_metadata_providers: &mut [&mut dyn CommitMetadataProvider],
) -> eyre::Result<Vec<StyledString>> {
    let root_oids = split_commit_graph_by_roots(effects, repo, dag, graph);
    let lines = get_output(
        effects.get_glyphs(),
        graph,
        commit_metadata_providers,
        head_oid,
        &root_oids,
    )?;
    Ok(lines)
}

/// Options for rendering the smartlog.
#[derive(Debug, Default)]
pub struct SmartlogOptions {
    /// Whether to also show commits in the smartlog which would normally not be
    /// visible.
    pub show_hidden_commits: bool,
}

/// Display a nice graph of commits you've recently worked on.
#[instrument]
pub fn smartlog(effects: &Effects, options: &SmartlogOptions) -> eyre::Result<()> {
    let SmartlogOptions {
        show_hidden_commits,
    } = options;

    let repo = Repo::from_current_dir()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let graph = make_smartlog_graph(
        effects,
        &repo,
        &dag,
        &event_replayer,
        event_cursor,
        !show_hidden_commits,
    )?;

    let lines = render_graph(
        effects,
        &repo,
        &dag,
        &graph,
        references_snapshot.head_oid,
        &mut [
            &mut CommitOidProvider::new(true)?,
            &mut RelativeTimeProvider::new(&repo, SystemTime::now())?,
            &mut ObsolescenceExplanationProvider::new(
                &event_replayer,
                event_replayer.make_default_cursor(),
            )?,
            &mut BranchesProvider::new(&repo, &references_snapshot)?,
            &mut DifferentialRevisionProvider::new(&repo)?,
            &mut CommitMessageProvider::new()?,
        ],
    )?;
    for line in lines {
        writeln!(
            effects.get_output_stream(),
            "{}",
            printable_styled_string(effects.get_glyphs(), line)?
        )?;
    }

    Ok(())
}
