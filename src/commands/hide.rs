//! Handle hiding commits when explicitly requested by the user (as opposed to
//! automatically as the result of a rewrite operation).

use std::collections::HashSet;
use std::time::SystemTime;

use crate::core::eventlog::{CommitVisibility, Event};
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::formatting::{printable_styled_string, Glyphs};
use crate::core::graph::{make_graph, BranchOids, CommitGraph, HeadOid, MainBranchOid, Node};
use crate::core::mergebase::MergeBaseDb;
use crate::core::metadata::{
    render_commit_metadata, CommitMessageProvider, CommitMetadataProvider, CommitOidProvider,
};
use crate::core::repo::Repo;
use crate::util::resolve_commits;
use crate::util::ResolveCommitsResult;
use crate::util::{get_branch_oid_to_names, get_db_conn};

fn recurse_on_commits_helper<
    'repo,
    'graph,
    Condition: Fn(&'graph Node<'repo>) -> bool,
    Callback: FnMut(&'graph Node<'repo>),
>(
    graph: &'graph CommitGraph<'repo>,
    condition: &Condition,
    commit: &git2::Commit<'repo>,
    callback: &mut Callback,
) {
    let node = &graph[&commit.id()];
    if condition(node) {
        callback(node);
    };

    for child_oid in node.children.iter() {
        let child_commit = &graph[&child_oid].commit;
        recurse_on_commits_helper(graph, condition, child_commit, callback)
    }
}

fn recurse_on_commits<'repo, F: Fn(&Node) -> bool>(
    repo: &'repo Repo,
    merge_base_db: &MergeBaseDb,
    event_replayer: &EventReplayer,
    commits: Vec<git2::Commit<'repo>>,
    condition: F,
) -> anyhow::Result<Vec<git2::Commit<'repo>>> {
    let head_oid = repo.get_head_oid()?;
    let main_branch_oid = repo.get_main_branch_oid()?;
    let branch_oid_to_names = get_branch_oid_to_names(repo)?;
    let graph = make_graph(
        repo,
        merge_base_db,
        event_replayer,
        event_replayer.make_default_cursor(),
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        false,
    )?;

    // Maintain ordering, since it's likely to be meaningful.
    let mut result: Vec<git2::Commit<'repo>> = Vec::new();
    let mut seen_oids = HashSet::new();
    for commit in commits {
        recurse_on_commits_helper(&graph, &condition, &commit, &mut |child_node| {
            let child_commit = &child_node.commit;
            if !seen_oids.contains(&child_commit.id()) {
                seen_oids.insert(child_commit.id());
                result.push(child_commit.clone());
            }
        });
    }
    Ok(result)
}

/// Hide the hashes provided on the command-line.
///
/// Args:
/// * `out`: The output stream to write to.
/// * `hashes`: A list of commit hashes to hide. Revs will be resolved (you can
///   provide an abbreviated commit hash or ref name).
/// * `recursive: If `true`, will recursively hide all children of the provided
///   commits as well.
///
/// Returns: exit code (0 denotes successful exit).
pub fn hide(hashes: Vec<String>, recursive: bool) -> anyhow::Result<isize> {
    let now = SystemTime::now();
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let conn = get_db_conn(&repo)?;
    let mut event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
    let merge_base_db = MergeBaseDb::new(&conn)?;

    let commits = resolve_commits(&repo, hashes)?;
    let commits = match commits {
        ResolveCommitsResult::Ok { commits } => commits,
        ResolveCommitsResult::CommitNotFound { commit: hash } => {
            println!("Commit not found: {}", hash);
            return Ok(1);
        }
    };
    let commits = if recursive {
        recurse_on_commits(&repo, &merge_base_db, &event_replayer, commits, |node| {
            node.is_visible
        })?
    } else {
        commits
    };

    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();
    let event_tx_id = event_log_db.make_transaction_id(now, "hide")?;
    let events = commits
        .iter()
        .map(|commit| Event::HideEvent {
            timestamp,
            event_tx_id,
            commit_oid: commit.id(),
        })
        .collect();
    event_log_db.add_events(events)?;

    let cursor = event_replayer.make_default_cursor();
    for commit in commits {
        let hidden_commit_text = {
            render_commit_metadata(
                &commit,
                &mut [
                    &mut CommitOidProvider::new(true)? as &mut dyn CommitMetadataProvider,
                    &mut CommitMessageProvider::new()?,
                ],
            )?
        };
        println!(
            "Hid commit: {}",
            printable_styled_string(&glyphs, hidden_commit_text)?
        );
        if let Some(CommitVisibility::Hidden) =
            event_replayer.get_cursor_commit_visibility(cursor, commit.id())
        {
            println!("(It was already hidden, so this operation had no effect.)");
        }

        let commit_target_oid =
            render_commit_metadata(&commit, &mut [&mut CommitOidProvider::new(false)?])?;
        println!(
            "To unhide this commit, run: git unhide {}",
            printable_styled_string(&glyphs, commit_target_oid)?
        );
    }

    Ok(0)
}

/// Unhide the hashes provided on the command-line.
///
/// Args:
/// * `out`: The output stream to write to.
/// * `hashes`: A list of commit hashes to unhide. Revs will be resolved (you can
/// provide an abbreviated commit hash or ref name).
/// * `recursive: If `true`, will recursively unhide all children of the provided
///   commits as well.
///
/// Returns: exit code (0 denotes successful exit).
pub fn unhide(hashes: Vec<String>, recursive: bool) -> anyhow::Result<isize> {
    let now = SystemTime::now();
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let conn = get_db_conn(&repo)?;
    let mut event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
    let merge_base_db = MergeBaseDb::new(&conn)?;

    let commits = resolve_commits(&repo, hashes)?;
    let commits = match commits {
        ResolveCommitsResult::Ok { commits } => commits,
        ResolveCommitsResult::CommitNotFound { commit: hash } => {
            println!("Commit not found: {}", hash);
            return Ok(1);
        }
    };
    let commits = if recursive {
        recurse_on_commits(&repo, &merge_base_db, &event_replayer, commits, |node| {
            !node.is_visible
        })?
    } else {
        commits
    };

    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();
    let event_tx_id = event_log_db.make_transaction_id(now, "unhide")?;
    let events = commits
        .iter()
        .map(|commit| Event::UnhideEvent {
            timestamp,
            event_tx_id,
            commit_oid: commit.id(),
        })
        .collect();
    event_log_db.add_events(events)?;

    let cursor = event_replayer.make_default_cursor();
    for commit in commits {
        let unhidden_commit_text = {
            render_commit_metadata(
                &commit,
                &mut [
                    &mut CommitOidProvider::new(true)?,
                    &mut CommitMessageProvider::new()?,
                ],
            )?
        };
        println!(
            "Unhid commit: {}",
            printable_styled_string(&glyphs, unhidden_commit_text)?
        );
        if let Some(CommitVisibility::Visible) =
            event_replayer.get_cursor_commit_visibility(cursor, commit.id())
        {
            println!("(It was not hidden, so this operation had no effect.)");
        }

        let commit_target_oid =
            render_commit_metadata(&commit, &mut [&mut CommitOidProvider::new(false)?])?;
        println!(
            "To hide this commit, run: git hide {}",
            printable_styled_string(&glyphs, commit_target_oid)?
        );
    }

    Ok(0)
}
