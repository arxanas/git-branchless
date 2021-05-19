//! Display a graph of commits that the user has worked on recently.
//!
//! The set of commits that are still being worked on is inferred from the event
//! log; see the `eventlog` module.

use std::cmp::Ordering;
use std::io::Write;
use std::time::SystemTime;

use fn_error_context::context;

use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::formatting::Glyphs;
use crate::core::graph::{make_graph, BranchOids, CommitGraph, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::core::metadata::{
    render_commit_metadata, BranchesProvider, CommitMessageProvider, CommitMetadataProvider,
    CommitOidProvider, DifferentialRevisionProvider, HiddenExplanationProvider,
    RelativeTimeProvider,
};
use crate::util::{
    get_branch_oid_to_names, get_db_conn, get_head_oid, get_main_branch_oid, get_repo,
};

/// Split fully-independent subgraphs into multiple graphs.
///
/// This is intended to handle the situation of having multiple lines of work
/// rooted from different commits in the main branch.
///
/// Returns the list such that the topologically-earlier subgraphs are first in
/// the list (i.e. those that would be rendered at the bottom of the smartlog).
fn split_commit_graph_by_roots(
    repo: &git2::Repository,
    merge_base_db: &MergeBaseDb,
    graph: &CommitGraph,
) -> Vec<git2::Oid> {
    let mut root_commit_oids: Vec<git2::Oid> = graph
        .iter()
        .filter(|(_oid, node)| node.parent.is_none())
        .map(|(oid, _node)| oid)
        .copied()
        .collect();

    let compare = |lhs_oid: &git2::Oid, rhs_oid: &git2::Oid| -> Ordering {
        let lhs_commit = repo.find_commit(*lhs_oid);
        let rhs_commit = repo.find_commit(*rhs_oid);

        let (lhs_commit, rhs_commit) = match (lhs_commit, rhs_commit) {
            (Err(_), Err(_)) | (Err(_), Ok(_)) | (Ok(_), Err(_)) => return lhs_oid.cmp(&rhs_oid),
            (Ok(lhs_commit), Ok(rhs_commit)) => (lhs_commit, rhs_commit),
        };

        let merge_base_oid = merge_base_db.get_merge_base_oid(repo, *lhs_oid, *rhs_oid);
        let merge_base_oid = match merge_base_oid {
            Err(_) => return lhs_oid.cmp(&rhs_oid),
            Ok(merge_base_oid) => merge_base_oid,
        };

        match merge_base_oid {
            // lhs was topologically first, so it should be sorted earlier in the list.
            Some(merge_base_oid) if merge_base_oid == *lhs_oid => Ordering::Less,
            Some(merge_base_oid) if merge_base_oid == *rhs_oid => Ordering::Greater,

            // The commits were not orderable (pathlogical situation). Let's
            // just order them by timestamp in that case to produce a consistent
            // and reasonable guess at the intended topological ordering.
            Some(_) | None => match lhs_commit.time().cmp(&rhs_commit.time()) {
                result @ Ordering::Less | result @ Ordering::Greater => result,
                Ordering::Equal => lhs_oid.cmp(&rhs_oid),
            },
        }
    };

    root_commit_oids.sort_by(compare);
    root_commit_oids
}

#[context("Getting child smartlog output for OID {:?}", &current_oid)]
fn get_child_output(
    glyphs: &Glyphs,
    graph: &CommitGraph,
    root_oids: &[git2::Oid],
    commit_metadata_providers: &[&dyn CommitMetadataProvider],
    head_oid: &HeadOid,
    current_oid: git2::Oid,
    last_child_line_char: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let current_node = &graph[&current_oid];
    let is_head = {
        let HeadOid(head_oid) = head_oid;
        Some(current_node.commit.id()) == *head_oid
    };

    let text = render_commit_metadata(&current_node.commit, commit_metadata_providers)?;
    let cursor = match (current_node.is_main, current_node.is_visible, is_head) {
        (false, false, false) => glyphs.commit_hidden,
        (false, false, true) => glyphs.commit_hidden_head,
        (false, true, false) => glyphs.commit_visible,
        (false, true, true) => glyphs.commit_visible_head,
        (true, false, false) => glyphs.commit_main_hidden,
        (true, false, true) => glyphs.commit_main_hidden_head,
        (true, true, false) => glyphs.commit_main,
        (true, true, true) => glyphs.commit_main_head,
    };

    let first_line = if is_head {
        format!(
            "{} {}",
            console::style(cursor).bold().to_string(),
            console::style(text).bold().to_string(),
        )
    } else {
        format!("{} {}", cursor, text)
    };

    let mut lines = vec![first_line];
    let mut children: Vec<_> = current_node
        .children
        .iter()
        .filter(|child_oid| graph.contains_key(child_oid))
        .cloned()
        .collect();
    children.sort_by_key(|child| graph[child].commit.time());
    for (child_idx, child_oid) in children.iter().enumerate() {
        if root_oids.contains(child_oid) {
            // Will be rendered by the parent.
            continue;
        }

        if child_idx == children.len() - 1 {
            match last_child_line_char {
                Some(_) => lines.push(format!("{}{}", glyphs.line_with_offshoot, glyphs.slash)),

                None => lines.push(glyphs.line.to_string()),
            }
        } else {
            lines.push(format!("{}{}", glyphs.line_with_offshoot, glyphs.slash))
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
            if child_idx == children.len() - 1 {
                match last_child_line_char {
                    Some(last_child_line_char) => {
                        lines.push(format!("{} {}", last_child_line_char, child_line))
                    }
                    None => lines.push(child_line),
                }
            } else {
                lines.push(format!("{} {}", glyphs.line, child_line))
            }
        }
    }
    Ok(lines)
}

/// Render a pretty graph starting from the given root OIDs in the given graph.
#[context(
    "Getting smartlog output for HEAD OID {:?}, root OIDs: {:?}",
    head_oid,
    root_oids
)]
fn get_output(
    glyphs: &Glyphs,
    graph: &CommitGraph,
    commit_metadata_providers: &[&dyn CommitMetadataProvider],
    head_oid: &HeadOid,
    root_oids: &[git2::Oid],
) -> anyhow::Result<Vec<String>> {
    let mut lines = Vec::new();

    // Determine if the provided OID has the provided parent OID as a parent.
    //
    // This returns `True` in strictly more cases than checking `graph`,
    // since there may be links between adjacent main branch commits which
    // are not reflected in `graph`.
    let has_real_parent = |oid: git2::Oid, parent_oid: git2::Oid| -> bool {
        graph[&oid]
            .commit
            .parent_ids()
            .any(|parent_oid2| parent_oid2 == parent_oid)
    };

    for (root_idx, root_oid) in root_oids.iter().enumerate() {
        let root_node = &graph[root_oid];
        if root_node.commit.parent_count() > 0 {
            if root_idx > 0 && has_real_parent(*root_oid, root_oids[root_idx - 1]) {
                lines.push(glyphs.line.to_owned())
            } else {
                lines.push(glyphs.vertical_ellipsis.to_owned())
            }
        } else if root_idx > 0 {
            // Pathological case: multiple topologically-unrelated roots.
            // Separate them with a newline.
            lines.push(String::new())
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
pub fn render_graph(
    out: &mut impl Write,
    glyphs: &Glyphs,
    repo: &git2::Repository,
    merge_base_db: &MergeBaseDb,
    graph: &CommitGraph,
    head_oid: &HeadOid,
    commit_metadata_providers: &[&dyn CommitMetadataProvider],
) -> anyhow::Result<()> {
    let root_oids = split_commit_graph_by_roots(repo, merge_base_db, graph);
    let lines = get_output(
        glyphs,
        graph,
        commit_metadata_providers,
        head_oid,
        &root_oids,
    )?;

    for line in lines {
        writeln!(out, "{}", line)?;
    }
    Ok(())
}

/// Display a nice graph of commits you've recently worked on.
pub fn smartlog(out: &mut impl Write) -> anyhow::Result<()> {
    let glyphs = Glyphs::detect();
    let repo = get_repo()?;
    let conn = get_db_conn(&repo)?;
    let merge_base_db = MergeBaseDb::new(&conn)?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
    let head_oid = get_head_oid(&repo)?;
    let main_branch_oid = get_main_branch_oid(&repo)?;
    let branch_oid_to_names = get_branch_oid_to_names(&repo)?;
    let graph = make_graph(
        &repo,
        &merge_base_db,
        &event_replayer,
        event_replayer.make_default_cursor(),
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().cloned().collect()),
        true,
    )?;

    render_graph(
        out,
        &glyphs,
        &repo,
        &merge_base_db,
        &graph,
        &HeadOid(head_oid),
        &[
            &CommitOidProvider::new(true)?,
            &RelativeTimeProvider::new(&repo, SystemTime::now())?,
            &HiddenExplanationProvider::new(
                &graph,
                &event_replayer,
                event_replayer.make_default_cursor(),
            )?,
            &BranchesProvider::new(&repo, &branch_oid_to_names)?,
            &DifferentialRevisionProvider::new(&repo)?,
            &CommitMessageProvider::new()?,
        ],
    )?;

    Ok(())
}
