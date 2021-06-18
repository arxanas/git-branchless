//! Utilities to deal with rewritten commits. See `Event::RewriteEvent` for
//! specifics on commit rewriting.

use std::collections::{HashMap, HashSet};

use anyhow::Context;
use cursive::utils::markup::StyledString;
use fn_error_context::context;
use indicatif::{ProgressBar, ProgressStyle};

use crate::core::formatting::printable_styled_string;
use crate::util::{run_git, run_hook, wrap_git_error, GitExecutable};

use super::eventlog::{Event, EventCursor, EventReplayer, EventTransactionId};
use super::formatting::Glyphs;
use super::graph::{find_path_to_merge_base, CommitGraph, MainBranchOid};
use super::mergebase::MergeBaseDb;
use super::metadata::{render_commit_metadata, CommitMessageProvider, CommitOidProvider};

/// For a rewritten commit, find the newest version of the commit.
///
/// For example, if we amend commit `abc` into commit `def1`, and then amend
/// `def1` into `def2`, then we can traverse the event log to find out that `def2`
/// is the newest version of `abc`.
///
/// If a commit was rewritten into itself through some chain of events, then
/// returns `None`, rather than the same commit OID.
pub fn find_rewrite_target(
    graph: &CommitGraph,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    oid: git2::Oid,
) -> Option<git2::Oid> {
    let event = event_replayer.get_cursor_commit_latest_event(event_cursor, oid);
    let event = match event {
        Some(event) => event,
        None => return None,
    };
    match event {
        Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid,
            new_commit_oid,
        } => {
            if *old_commit_oid == oid && *new_commit_oid != oid {
                let possible_newer_oid =
                    find_rewrite_target(graph, event_replayer, event_cursor, *new_commit_oid);
                match possible_newer_oid {
                    Some(newer_commit_oid) => Some(newer_commit_oid),
                    None => Some(*new_commit_oid),
                }
            } else {
                None
            }
        }

        Event::RefUpdateEvent { .. }
        | Event::CommitEvent { .. }
        | Event::HideEvent { .. }
        | Event::UnhideEvent { .. } => None,
    }
}

/// Find commits which have been "abandoned" in the commit graph.
///
/// A commit is considered "abandoned" if it is visible, but one of its parents
/// is hidden.
pub fn find_abandoned_children(
    graph: &CommitGraph,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    oid: git2::Oid,
) -> Option<(git2::Oid, Vec<git2::Oid>)> {
    let rewritten_oid = find_rewrite_target(graph, event_replayer, event_cursor, oid)?;

    // Adjacent main branch commits are not linked in the commit graph, but if
    // the user rewrote a main branch commit, then we may need to restack
    // subsequent main branch commits. Find the real set of children commits so
    // that we can do this.
    let mut real_children_oids = graph[&oid].children.clone();
    let additional_children_oids: HashSet<git2::Oid> = graph
        .iter()
        .filter_map(|(possible_child_oid, possible_child_node)| {
            if real_children_oids.contains(possible_child_oid) {
                // Don't bother looking up the parents for commits we are
                // already including.
                None
            } else if possible_child_node
                .commit
                .parent_ids()
                .any(|parent_oid| parent_oid == oid)
            {
                Some(possible_child_oid)
            } else {
                None
            }
        })
        .copied()
        .collect();
    real_children_oids.extend(additional_children_oids);

    let visible_children_oids = real_children_oids
        .iter()
        .filter(|child_oid| graph[child_oid].is_visible)
        .copied()
        .collect();
    Some((rewritten_oid, visible_children_oids))
}

fn make_label_name(repo: &git2::Repository, mut preferred_name: String) -> anyhow::Result<String> {
    match repo.find_reference(&format!("refs/rewritten/{}", preferred_name)) {
        Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(preferred_name),
        Ok(_) => {
            preferred_name.push('\'');
            make_label_name(repo, preferred_name)
        }
        Err(err) => Err(wrap_git_error(err)),
    }
}

#[derive(Debug)]
enum RebaseCommand {
    Label { label_name: String },
    Reset { label_name: String },
    Pick { commit_oid: git2::Oid },
}

/// Represents a sequence of commands that can be executed to carry out a rebase
/// operation.
#[derive(Debug)]
pub struct RebasePlan {
    commands: Vec<RebaseCommand>,
}

impl ToString for RebaseCommand {
    fn to_string(&self) -> String {
        match self {
            RebaseCommand::Label { label_name } => format!("label {}", label_name),
            RebaseCommand::Reset { label_name } => format!("reset {}", label_name),
            RebaseCommand::Pick { commit_oid } => format!("pick {}", commit_oid),
        }
    }
}

fn make_rebase_plan_for_current_commit(
    repo: &git2::Repository,
    graph: &CommitGraph,
    current_oid: git2::Oid,
    current_label: &str,
    mut acc: Vec<RebaseCommand>,
) -> anyhow::Result<Vec<RebaseCommand>> {
    let acc = {
        acc.push(RebaseCommand::Pick {
            commit_oid: current_oid,
        });
        acc
    };
    let current_node = match graph.get(&current_oid) {
        Some(current_node) => current_node,
        None => {
            anyhow::bail!(format!(
                "BUG: commit {} could not be found in the commit graph",
                current_oid.to_string()
            ))
        }
    };

    let children = {
        // Sort for determinism.
        let mut children = current_node.children.iter().copied().collect::<Vec<_>>();
        children.sort_by_key(|child_oid| (graph[child_oid].commit.time(), child_oid.to_string()));
        children
    };
    match children.as_slice() {
        [] => Ok(acc),
        [only_child_oid] => {
            let acc = make_rebase_plan_for_current_commit(
                repo,
                &graph,
                *only_child_oid,
                current_label,
                acc,
            )?;
            Ok(acc)
        }
        children => {
            let command_num = acc.len();
            let label_name = make_label_name(repo, format!("label-{}", command_num))?;
            let mut acc = acc;
            acc.push(RebaseCommand::Label {
                label_name: label_name.clone(),
            });
            for child_oid in children {
                acc =
                    make_rebase_plan_for_current_commit(repo, graph, *child_oid, &label_name, acc)?;
                acc.push(RebaseCommand::Reset {
                    label_name: label_name.clone(),
                });
            }
            Ok(acc)
        }
    }
}

/// Generate a sequence of rebase steps that cause the subtree at `source_oid`
/// to be rebased on top of the commit at `main_branch_oid`.
pub fn make_rebase_plan(
    repo: &git2::Repository,
    merge_base_db: &MergeBaseDb,
    graph: &CommitGraph,
    main_branch_oid: &MainBranchOid,
    source_oid: git2::Oid,
) -> anyhow::Result<RebasePlan> {
    let label_name = make_label_name(&repo, "onto".to_string())?;
    let (source_oid, mut commands) = {
        let MainBranchOid(main_branch_oid) = main_branch_oid;
        let merge_base_oid =
            merge_base_db.get_merge_base_oid(&repo, *main_branch_oid, source_oid)?;
        if merge_base_oid == Some(source_oid) {
            // In this case, the `source` OID is an ancestor of the main branch.
            // This means that the user is trying to rewrite public history,
            // which is typically not recommended, but let's try to do it
            // anyways.
            let path =
                find_path_to_merge_base(&repo, &merge_base_db, *main_branch_oid, source_oid)?
                    .unwrap_or_default();
            let commands: Vec<RebaseCommand> = path
                .into_iter()
                // Skip the first element, which is the main branch OID, since
                // it'll be picked as part of the recursive rebase plan below.
                .skip(1)
                // Reverse the path, since it goes from main branch OID to
                // source OID, but we want a path from source OID to main branch
                // OID.
                .rev()
                .map(|main_branch_commit| RebaseCommand::Pick {
                    commit_oid: main_branch_commit.id(),
                })
                .collect();
            (*main_branch_oid, commands)
        } else {
            (source_oid, Vec::new())
        }
    };
    commands.push(RebaseCommand::Label {
        label_name: label_name.clone(),
    });
    let commands =
        make_rebase_plan_for_current_commit(repo, graph, source_oid, &label_name, commands)?;
    Ok(RebasePlan { commands })
}

enum RebaseInMemoryResult {
    Succeeded {
        rewritten_oids: Vec<(git2::Oid, git2::Oid)>,
    },
    CannotRebaseMergeCommit {
        commit_oid: git2::Oid,
    },
    MergeConflict {
        commit_oid: git2::Oid,
    },
}

#[context("Rebasing in memory onto to {}", dest_oid.to_string())]
fn rebase_in_memory(
    glyphs: &Glyphs,
    repo: &git2::Repository,
    rebase_plan: &RebasePlan,
    dest_oid: git2::Oid,
) -> anyhow::Result<RebaseInMemoryResult> {
    let mut current_oid = dest_oid;
    let mut labels: HashMap<String, git2::Oid> = HashMap::new();
    let mut rewritten_oids = Vec::new();

    let mut i = 0;
    let num_picks = rebase_plan
        .commands
        .iter()
        .filter(|command| match command {
            RebaseCommand::Label { .. } | RebaseCommand::Reset { .. } => false,
            RebaseCommand::Pick { .. } => true,
        })
        .count();

    for command in rebase_plan.commands.iter() {
        match command {
            RebaseCommand::Label { label_name } => {
                labels.insert(label_name.clone(), current_oid);
            }
            RebaseCommand::Reset { label_name } => {
                current_oid = match labels.get(label_name) {
                    Some(oid) => *oid,
                    None => anyhow::bail!("BUG: no associated OID for label: {}", label_name),
                };
            }
            RebaseCommand::Pick { commit_oid } => {
                let current_commit = repo
                    .find_commit(current_oid)
                    .with_context(|| format!("Finding current commit by OID: {:?}", current_oid))?;
                let commit_to_apply = repo
                    .find_commit(*commit_oid)
                    .with_context(|| format!("Finding commit to apply by OID: {:?}", commit_oid))?;
                i += 1;

                let commit_description =
                    printable_styled_string(glyphs, friendly_describe_commit(repo, *commit_oid)?)?;
                let template = format!("[{}/{}] {{spinner}} {{wide_msg}}", i, num_picks);
                let progress = ProgressBar::new_spinner();
                progress.set_style(ProgressStyle::default_spinner().template(&template.trim()));
                progress.set_message("Starting");
                progress.enable_steady_tick(100);

                if commit_to_apply.parent_count() > 1 {
                    return Ok(RebaseInMemoryResult::CannotRebaseMergeCommit {
                        commit_oid: *commit_oid,
                    });
                };

                progress.set_message(format!("Applying patch for commit: {}", commit_description));
                let mut rebased_index =
                    repo.cherrypick_commit(&commit_to_apply, &current_commit, 0, None)?;

                progress.set_message(format!(
                    "Checking for merge conflicts: {}",
                    commit_description
                ));
                if rebased_index.has_conflicts() {
                    return Ok(RebaseInMemoryResult::MergeConflict {
                        commit_oid: *commit_oid,
                    });
                }

                progress.set_message(format!(
                    "Writing commit data to disk: {}",
                    commit_description
                ));
                let commit_tree_oid = rebased_index
                    .write_tree_to(repo)
                    .with_context(|| "Converting index to tree")?;
                let commit_tree = repo
                    .find_tree(commit_tree_oid)
                    .with_context(|| "Looking up freshly-written tree")?;
                let commit_message = match commit_to_apply.message_raw() {
                    Some(message) => message,
                    None => anyhow::bail!(
                        "Could not decode commit message for commit: {:?}",
                        commit_oid
                    ),
                };

                progress.set_message(format!("Committing to repository: {}", commit_description));
                let rebased_commit_oid = repo
                    .commit(
                        None,
                        &commit_to_apply.author(),
                        &commit_to_apply.committer(),
                        commit_message,
                        &commit_tree,
                        &[&current_commit],
                    )
                    .with_context(|| "Applying rebased commit")?;
                rewritten_oids.push((*commit_oid, rebased_commit_oid));
                current_oid = rebased_commit_oid;

                let commit_description = printable_styled_string(
                    glyphs,
                    friendly_describe_commit(repo, rebased_commit_oid)?,
                )?;
                progress.finish_with_message(format!("Committed as: {}", commit_description));
            }
        }
    }

    Ok(RebaseInMemoryResult::Succeeded { rewritten_oids })
}

fn post_rebase_in_memory(
    git_executable: &GitExecutable,
    repo: &git2::Repository,
    rewritten_oids: &[(git2::Oid, git2::Oid)],
    event_tx_id: EventTransactionId,
) -> anyhow::Result<isize> {
    let stdin = rewritten_oids
        .iter()
        .map(|(old_oid, new_oid)| format!("{} {}\n", old_oid.to_string(), new_oid.to_string()))
        .collect::<String>();
    run_hook(repo, "post-rewrite", event_tx_id, &["rebase"], Some(stdin))?;

    // TODO: move any affected branches, to match the behavior of `git rebase`.

    let head_oid = repo.head()?.peel_to_commit()?.id();
    if let Some(new_head_oid) = rewritten_oids.iter().find_map(|(old_oid, new_oid)| {
        if *old_oid == head_oid {
            Some(new_oid)
        } else {
            None
        }
    }) {
        let result = run_git(
            git_executable,
            Some(event_tx_id),
            &["checkout", &new_head_oid.to_string()],
        )?;
        if result != 0 {
            return Ok(result);
        }
    }

    Ok(0)
}

#[context("Rebasing on disk from {} to {}", source_oid.to_string(), dest_oid.to_string())]
fn rebase_on_disk(
    git_executable: &GitExecutable,
    repo: &git2::Repository,
    rebase_plan: &RebasePlan,
    source_oid: git2::Oid,
    dest_oid: git2::Oid,
    event_tx_id: EventTransactionId,
) -> anyhow::Result<isize> {
    let progress = ProgressBar::new_spinner();
    progress.enable_steady_tick(100);
    progress.set_message("Initializing rebase");

    // Attempt to initialize a new rebase. However, `git2` doesn't support the
    // commands we need (`label` and `reset`), so we won't be using it for the
    // actual rebase process.
    let _rebase: git2::Rebase = repo
        .rebase(
            None,
            // TODO: if the target was a branch, do we need to use an annotated
            // commit which was instantiated from the branch?
            Some(&repo.find_annotated_commit(source_oid)?),
            Some(&repo.find_annotated_commit(dest_oid)?),
            None,
        )
        .with_context(|| "Setting up rebase to write `git-rebase-todo`")?;

    let todo_file = repo.path().join("rebase-merge").join("git-rebase-todo");
    std::fs::write(
        todo_file.as_path(),
        rebase_plan
            .commands
            .iter()
            .map(|command| format!("{}\n", command.to_string()))
            .collect::<String>(),
    )
    .with_context(|| format!("Writing `git-rebase-todo` to: {:?}", todo_file.as_path()))?;
    let end_file = repo.path().join("rebase-merge").join("end");
    std::fs::write(
        end_file.as_path(),
        format!("{}\n", rebase_plan.commands.len()),
    )
    .with_context(|| format!("Writing `end` to: {:?}", end_file.as_path()))?;

    progress.set_message("Calling Git for on-disk rebase");
    let result = run_git(
        &git_executable,
        Some(event_tx_id),
        &["rebase", "--continue"],
    )?;

    Ok(result)
}

#[context("Describing commit {}", commit_oid.to_string())]
fn friendly_describe_commit(
    repo: &git2::Repository,
    commit_oid: git2::Oid,
) -> anyhow::Result<StyledString> {
    let commit = repo
        .find_commit(commit_oid)
        .with_context(|| "Looking up commit to describe")?;
    let description = render_commit_metadata(
        &commit,
        &mut [
            &mut CommitOidProvider::new(true)?,
            &mut CommitMessageProvider::new()?,
        ],
    )?;
    Ok(description)
}

/// Execute the provided rebase plan. Returns the exit status (zero indicates
/// success).
pub fn execute_rebase_plan(
    glyphs: &Glyphs,
    git_executable: &GitExecutable,
    repo: &git2::Repository,
    event_tx_id: EventTransactionId,
    rebase_plan: &RebasePlan,
    source_oid: git2::Oid,
    dest_oid: git2::Oid,
    force_on_disk: bool,
) -> anyhow::Result<isize> {
    if !force_on_disk {
        println!("Attempting rebase in-memory...");
        match rebase_in_memory(glyphs, &repo, &rebase_plan, dest_oid)? {
            RebaseInMemoryResult::Succeeded { rewritten_oids } => {
                post_rebase_in_memory(git_executable, repo, &rewritten_oids, event_tx_id)?;
                println!("In-memory rebase succeeded.");
                return Ok(0);
            }
            RebaseInMemoryResult::CannotRebaseMergeCommit { commit_oid } => {
                println!(
                    "Merge commits currently can't be rebased with `git move`. The merge commit was: {}",
                    printable_styled_string(glyphs, friendly_describe_commit(repo, commit_oid)?)?
                );
                return Ok(1);
            }
            RebaseInMemoryResult::MergeConflict { commit_oid } => {
                println!(
                    "Merge conflict, falling back to rebase on-disk. The conflicting commit was: {}",
                    printable_styled_string(glyphs, friendly_describe_commit(repo, commit_oid)?)?,
                );
            }
        }
    }

    let result = rebase_on_disk(
        git_executable,
        repo,
        &rebase_plan,
        source_oid,
        dest_oid,
        event_tx_id,
    )?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use crate::core::eventlog::EventLogDb;
    use crate::core::graph::{make_graph, BranchOids, HeadOid, MainBranchOid};
    use crate::core::mergebase::MergeBaseDb;
    use crate::testing::{with_git, Git, GitRunOptions};
    use crate::util::{get_branch_oid_to_names, get_db_conn, get_head_oid, get_main_branch_oid};

    use super::*;

    fn find_rewrite_target_helper(git: &Git, oid: git2::Oid) -> anyhow::Result<Option<git2::Oid>> {
        let repo = git.get_repo()?;
        let conn = get_db_conn(&repo)?;
        let merge_base_db = MergeBaseDb::new(&conn)?;
        let event_log_db = EventLogDb::new(&conn)?;
        let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
        let event_cursor = event_replayer.make_default_cursor();
        let head_oid = get_head_oid(&repo)?;
        let main_branch_oid = get_main_branch_oid(&repo)?;
        let branch_oid_to_names = get_branch_oid_to_names(&repo)?;
        let graph = make_graph(
            &repo,
            &merge_base_db,
            &event_replayer,
            event_cursor,
            &HeadOid(head_oid),
            &MainBranchOid(main_branch_oid),
            &BranchOids(branch_oid_to_names.keys().copied().collect()),
            true,
        )?;

        let rewrite_target = find_rewrite_target(&graph, &event_replayer, event_cursor, oid);
        Ok(rewrite_target)
    }

    #[test]
    fn test_find_rewrite_target() -> anyhow::Result<()> {
        with_git(|git| {
            git.init_repo()?;
            let commit_time = 1;
            let old_oid = git.commit_file("test1", commit_time)?;

            {
                git.run(&["commit", "--amend", "-m", "test1 amended once"])?;
                let new_oid: git2::Oid = {
                    let (stdout, _stderr) = git.run(&["rev-parse", "HEAD"])?;
                    stdout.trim().parse()?
                };
                let rewrite_target = find_rewrite_target_helper(&git, old_oid)?;
                assert_eq!(rewrite_target, Some(new_oid));
            }

            {
                git.run(&["commit", "--amend", "-m", "test1 amended twice"])?;
                let new_oid: git2::Oid = {
                    let (stdout, _stderr) = git.run(&["rev-parse", "HEAD"])?;
                    stdout.trim().parse()?
                };
                let rewrite_target = find_rewrite_target_helper(&git, old_oid)?;
                assert_eq!(rewrite_target, Some(new_oid));
            }

            {
                git.run_with_options(
                    &["commit", "--amend", "-m", "create test1.txt"],
                    &GitRunOptions {
                        time: commit_time,
                        ..Default::default()
                    },
                )?;
                let rewrite_target = find_rewrite_target_helper(&git, old_oid)?;
                assert_eq!(rewrite_target, None);
            }

            Ok(())
        })
    }
}
