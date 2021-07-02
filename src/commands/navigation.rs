//! Convenience commands to help the user move through a stack of commits.

use std::collections::HashMap;

use log::warn;

use crate::commands::smartlog::smartlog;
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::formatting::{printable_styled_string, Glyphs};
use crate::core::graph::{
    find_path_to_merge_base, make_graph, BranchOids, HeadOid, MainBranchOid, Node,
};
use crate::core::mergebase::MergeBaseDb;
use crate::core::metadata::{render_commit_metadata, CommitMessageProvider, CommitOidProvider};
use crate::git::Repo;
use crate::util::{run_git, GitRunInfo};

/// Go back a certain number of commits.
pub fn prev(git_run_info: &GitRunInfo, num_commits: Option<isize>) -> anyhow::Result<isize> {
    let exit_code = match num_commits {
        None => run_git(git_run_info, None, &["checkout", "HEAD^"])?,
        Some(num_commits) => run_git(
            git_run_info,
            None,
            &["checkout", &format!("HEAD~{}", num_commits)],
        )?,
    };
    if exit_code != 0 {
        return Ok(exit_code);
    }
    smartlog()?;
    Ok(0)
}

/// Some commits have multiple children, which makes `next` ambiguous. These
/// values disambiguate which child commit to go to, according to the committed
/// date.
#[derive(Clone, Copy, Debug)]
pub enum Towards {
    /// When encountering multiple children, select the newest one.
    Newest,

    /// When encountering multiple children, select the oldest one.
    Oldest,
}

fn advance_towards_main_branch(
    repo: &git2::Repository,
    merge_base_db: &MergeBaseDb,
    graph: &HashMap<git2::Oid, Node>,
    current_oid: git2::Oid,
    main_branch_oid: &MainBranchOid,
) -> anyhow::Result<(isize, git2::Oid)> {
    let MainBranchOid(main_branch_oid) = main_branch_oid;
    let path = find_path_to_merge_base(repo, merge_base_db, *main_branch_oid, current_oid)?;
    let path = match path {
        None => return Ok((0, current_oid)),
        Some(path) if path.len() == 1 => {
            // Must be the case that `current_oid == main_branch_oid`.
            return Ok((0, current_oid));
        }
        Some(path) => path,
    };

    for (i, commit) in (1..).zip(path.iter().rev().skip(1)) {
        if graph.contains_key(&commit.id()) {
            return Ok((i, commit.id()));
        }
    }

    warn!("Failed to find graph commit when advancing towards main branch");
    Ok((0, current_oid))
}

fn advance_towards_own_commit(
    glyphs: &Glyphs,
    repo: &git2::Repository,
    graph: &HashMap<git2::Oid, Node>,
    current_oid: git2::Oid,
    num_commits: isize,
    towards: Option<Towards>,
) -> anyhow::Result<Option<git2::Oid>> {
    let mut current_oid = current_oid;
    for i in 0..num_commits {
        let children = &graph[&current_oid].children;
        current_oid = match (towards, children.as_slice()) {
            (_, []) => {
                // It would also make sense to issue an error here, rather than
                // silently stop going forward commits.
                break;
            }
            (_, [only_child_oid]) => *only_child_oid,
            (Some(Towards::Newest), [.., newest_child_oid]) => *newest_child_oid,
            (Some(Towards::Oldest), [oldest_child_oid, ..]) => *oldest_child_oid,
            (None, [_, _, ..]) => {
                println!(
                    "Found multiple possible next commits to go to after traversing {} children:",
                    i
                );

                for (j, child_oid) in (0..).zip(children.iter()) {
                    let descriptor = if j == 0 {
                        " (oldest)"
                    } else if j + 1 == children.len() {
                        " (newest)"
                    } else {
                        ""
                    };

                    let commit_text = render_commit_metadata(
                        &repo.find_commit(*child_oid)?,
                        &mut [
                            &mut CommitOidProvider::new(true)?,
                            &mut CommitMessageProvider::new()?,
                        ],
                    )?;
                    println!(
                        "  {} {}{}",
                        glyphs.bullet_point,
                        printable_styled_string(&glyphs, commit_text)?,
                        descriptor
                    );
                }
                println!("(Pass --oldest (-o) or --newest (-n) to select between ambiguous next commits)");
                return Ok(None);
            }
        };
    }
    Ok(Some(current_oid))
}

/// Go forward a certain number of commits.
pub fn next(
    git_run_info: &GitRunInfo,
    num_commits: Option<isize>,
    towards: Option<Towards>,
) -> anyhow::Result<isize> {
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let merge_base_db = MergeBaseDb::new(&conn)?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;

    let head_oid = match repo.get_head_info()?.oid {
        Some(head_oid) => head_oid,
        None => anyhow::bail!("No HEAD present; cannot calculate next commit"),
    };
    let main_branch_oid = repo.get_main_branch_oid()?;
    let branch_oid_to_names = repo.get_branch_oid_to_names()?;
    let graph = make_graph(
        &repo,
        &merge_base_db,
        &event_replayer,
        event_replayer.make_default_cursor(),
        &HeadOid(Some(head_oid)),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        true,
    )?;

    let num_commits = num_commits.unwrap_or(1);
    let (num_commits_traversed_towards_main_branch, current_oid) = advance_towards_main_branch(
        &repo,
        &merge_base_db,
        &graph,
        head_oid,
        &MainBranchOid(main_branch_oid),
    )?;
    let num_commits = num_commits - num_commits_traversed_towards_main_branch;
    let current_oid =
        advance_towards_own_commit(&glyphs, &repo, &graph, current_oid, num_commits, towards)?;
    let current_oid = match current_oid {
        None => return Ok(1),
        Some(current_oid) => current_oid,
    };

    let result = run_git(git_run_info, None, &["checkout", &current_oid.to_string()])?;
    if result != 0 {
        return Ok(result);
    }

    smartlog()?;
    Ok(0)
}
