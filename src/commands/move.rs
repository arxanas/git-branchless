//! Move commits and subtrees from one place to another.
//!
//! Under the hood, this makes use of Git's advanced rebase functionality, which
//! is also used to preserve merge commits using the `--rebase-merges` option.

use std::collections::HashMap;
use std::io::Write;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::time::SystemTime;

use anyhow::Context;
use fn_error_context::context;

use crate::core::eventlog::EventTransactionId;
use crate::core::eventlog::BRANCHLESS_TRANSACTION_ID_ENV_VAR;
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::graph::{make_graph, BranchOids, CommitGraph, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::util::{
    get_branch_oid_to_names, get_db_conn, get_head_oid, get_repo, resolve_commits, run_git,
    wrap_git_error, GitExecutable, ResolveCommitsResult,
};

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

enum RebaseCommand {
    Label { label_name: String },
    Reset { label_name: String },
    Pick { commit_oid: git2::Oid },
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

fn plan_rebase_current(
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
            anyhow::bail!(format!("Commit {} could not be found. `git move` cannot currently move between commits which are not shown in the smartlog.", current_oid.to_string()))
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
            let acc = plan_rebase_current(repo, &graph, *only_child_oid, current_label, acc)?;
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
                acc = plan_rebase_current(repo, graph, *child_oid, &label_name, acc)?;
                acc.push(RebaseCommand::Reset {
                    label_name: label_name.clone(),
                });
            }
            Ok(acc)
        }
    }
}

fn make_rebase_plan(
    repo: &git2::Repository,
    graph: &CommitGraph,
    source: git2::Oid,
) -> anyhow::Result<Vec<RebaseCommand>> {
    let label_name = make_label_name(&repo, "onto".to_string())?;
    let result = vec![RebaseCommand::Label {
        label_name: label_name.clone(),
    }];
    let result = plan_rebase_current(repo, graph, source, &label_name, result)?;
    Ok(result)
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

fn rebase_in_memory(
    out: &mut impl Write,
    repo: &git2::Repository,
    rebase_plan: &[RebaseCommand],
    dest: git2::Oid,
) -> anyhow::Result<RebaseInMemoryResult> {
    let mut current_oid = dest;
    let mut labels: HashMap<String, git2::Oid> = HashMap::new();
    let mut rewritten_oids = Vec::new();

    let mut i = 0;
    let num_picks = rebase_plan
        .iter()
        .filter(|command| match command {
            RebaseCommand::Label { .. } | RebaseCommand::Reset { .. } => false,
            RebaseCommand::Pick { .. } => true,
        })
        .count();

    for command in rebase_plan {
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
                writeln!(
                    out,
                    "Rebase in-memory ({}/{}): {}",
                    i,
                    num_picks,
                    commit_to_apply
                        .summary()
                        .unwrap_or("<no summary available>")
                )?;

                let commit_to_apply_tree = commit_to_apply.tree().with_context(|| {
                    format!(
                        "Getting tree for commit to apply: {}",
                        commit_oid.to_string()
                    )
                })?;
                let diff = match commit_to_apply.parent_count() {
                    0 => repo
                        .diff_tree_to_tree(None, Some(&commit_to_apply_tree), None)
                        .with_context(|| "Diffing commit with no parents")?,
                    1 => {
                        let parent_commit = commit_to_apply.parent(0).with_context(|| {
                            format!(
                                "Getting parent commit for commit: {}",
                                current_oid.to_string()
                            )
                        })?;
                        let parent_commit_tree = parent_commit.tree().with_context(|| {
                            format!(
                                "Getting tree for parent commit: {}",
                                parent_commit.id().to_string()
                            )
                        })?;
                        repo.diff_tree_to_tree(
                            Some(&parent_commit_tree),
                            Some(&commit_to_apply_tree),
                            None,
                        )
                        .with_context(|| {
                            format!("Diffing commit against parent: {}", current_oid.to_string(),)
                        })?
                    }
                    _ => {
                        return Ok(RebaseInMemoryResult::CannotRebaseMergeCommit {
                            commit_oid: *commit_oid,
                        });
                    }
                };
                let current_commit_tree = current_commit.tree().with_context(|| {
                    format!(
                        "Getting tree for current commit: {}",
                        current_oid.to_string()
                    )
                })?;
                let mut rebased_index = repo.apply_to_tree(&current_commit_tree, &diff, None)?;
                if rebased_index.has_conflicts() {
                    return Ok(RebaseInMemoryResult::MergeConflict {
                        commit_oid: *commit_oid,
                    });
                }

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
            }
        }
    }

    Ok(RebaseInMemoryResult::Succeeded { rewritten_oids })
}

fn post_rebase_in_memory(
    repo: &git2::Repository,
    rewritten_oids: &[(git2::Oid, git2::Oid)],
    event_tx_id: EventTransactionId,
) -> anyhow::Result<()> {
    let post_rewrite_hook_path = repo
        .config()?
        .get_path("core.hooksPath")
        .unwrap_or_else(|_| repo.path().join("hooks"))
        .join("post-rewrite");
    if post_rewrite_hook_path.exists() {
        let mut child = Command::new(post_rewrite_hook_path.as_path())
            .arg("rebase")
            .env(BRANCHLESS_TRANSACTION_ID_ENV_VAR, event_tx_id.to_string())
            .stdin(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "Invoking post-rewrite hook at: {:?}",
                    post_rewrite_hook_path.as_path()
                )
            })?;

        let stdin = child.stdin.as_mut().unwrap();
        for (old_oid, new_oid) in rewritten_oids {
            writeln!(stdin, "{} {}", old_oid.to_string(), new_oid.to_string())?;
        }

        let _ignored: ExitStatus = child.wait()?;
    }

    // TODO: move any affected branches, to match the behavior of `git rebase`.

    Ok(())
}

#[context("Rebasing on disk from {} to {}", source.to_string(), dest.to_string())]
fn rebase_on_disk(
    out: &mut impl Write,
    err: &mut impl Write,
    git_executable: &GitExecutable,
    repo: &git2::Repository,
    rebase_plan: &[RebaseCommand],
    source: git2::Oid,
    dest: git2::Oid,
    event_tx_id: EventTransactionId,
) -> anyhow::Result<isize> {
    // Attempt to initialize a new rebase. However, `git2` doesn't support the
    // commands we need (`label` and `reset`), so we won't be using it for the
    // actual rebase process.
    let _rebase: git2::Rebase = repo
        .rebase(
            None,
            // TODO: if the target was a branch, do we need to use an annotated
            // commit which was instantiated from the branch?
            Some(&repo.find_annotated_commit(source)?),
            Some(&repo.find_annotated_commit(dest)?),
            None,
        )
        .with_context(|| "Setting up rebase to write `git-rebase-todo`")?;

    let todo_file = repo.path().join("rebase-merge").join("git-rebase-todo");
    std::fs::write(
        todo_file.as_path(),
        rebase_plan
            .iter()
            .map(|command| format!("{}\n", command.to_string()))
            .collect::<String>(),
    )
    .with_context(|| format!("Writing `git-rebase-todo` to: {:?}", todo_file.as_path()))?;
    let end_file = repo.path().join("rebase-merge").join("end");
    std::fs::write(end_file.as_path(), format!("{}\n", rebase_plan.len()))
        .with_context(|| format!("Writing `end` to: {:?}", end_file.as_path()))?;

    let result = run_git(
        out,
        err,
        &git_executable,
        Some(event_tx_id),
        &["rebase", "--continue"],
    )?;

    Ok(result)
}

#[context("Moving subtree from {} to {}", source.to_string(), dest.to_string())]
fn move_subtree(
    out: &mut impl Write,
    err: &mut impl Write,
    git_executable: &GitExecutable,
    repo: &git2::Repository,
    graph: &CommitGraph,
    event_tx_id: EventTransactionId,
    source: git2::Oid,
    dest: git2::Oid,
    force_on_disk: bool,
) -> anyhow::Result<isize> {
    // Make the rebase plan here so that it doesn't potentially fail after the
    // rebase has been initialized.
    let rebase_plan = make_rebase_plan(&repo, &graph, source)?;

    if !force_on_disk {
        writeln!(out, "Attempting rebase in-memory...")?;
        match rebase_in_memory(out, &repo, &rebase_plan, dest)? {
            RebaseInMemoryResult::Succeeded { rewritten_oids } => {
                post_rebase_in_memory(&repo, &rewritten_oids, event_tx_id)?;
                writeln!(out, "In-memory rebase succeeded.")?;
                return Ok(0);
            }
            RebaseInMemoryResult::CannotRebaseMergeCommit { commit_oid } => {
                writeln!(
                    out,
                    "Merge commits cannot be rebased in-memory. The commit was: {}",
                    commit_oid.to_string()
                )?;
            }
            RebaseInMemoryResult::MergeConflict { commit_oid } => {
                writeln!(
                    out,
                    "Merge conflict, falling back to rebase on-disk. The conflicting commit was: {}",
                    commit_oid.to_string()
                )?;
            }
        }
    }

    let result = rebase_on_disk(
        out,
        err,
        git_executable,
        repo,
        &rebase_plan,
        source,
        dest,
        event_tx_id,
    )?;
    Ok(result)
}

/// Move a subtree from one place to another.
pub fn r#move(
    out: &mut impl Write,
    err: &mut impl Write,
    git_executable: &GitExecutable,
    source: Option<String>,
    dest: Option<String>,
    force_on_disk: bool,
) -> anyhow::Result<isize> {
    let repo = get_repo()?;
    let head_oid = get_head_oid(&repo)?;
    let source = match source {
        Some(source) => source,
        None => head_oid
            .expect(
                "No --source argument was provided, and no OID for HEAD is available as a default",
            )
            .to_string(),
    };
    let dest = match dest {
        Some(dest) => dest,
        None => head_oid
            .expect(
                "No --dest argument was provided, and no OID for HEAD is available as a default",
            )
            .to_string(),
    };
    let (source_oid, dest_oid) = match resolve_commits(&repo, vec![source, dest])? {
        ResolveCommitsResult::Ok { commits } => match &commits.as_slice() {
            [source_commit, dest_commit] => (source_commit.id(), dest_commit.id()),
            _ => anyhow::bail!("Unexpected number of returns values from resolve_commits"),
        },
        ResolveCommitsResult::CommitNotFound { commit } => {
            writeln!(out, "Commit not found: {}", commit)?;
            return Ok(1);
        }
    };

    let branch_oid_to_names = get_branch_oid_to_names(&repo)?;
    let conn = get_db_conn(&repo)?;
    let merge_base_db = MergeBaseDb::new(&conn)?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let graph = make_graph(
        &repo,
        &merge_base_db,
        &event_replayer,
        event_cursor,
        &HeadOid(head_oid),
        // We need to make sure that all the descendants of the source commit
        // are available in the commit graph. To do so, we pretend that it's the
        // main branch OID. This ensures that there's a path from every node in
        // the commit graph to the source node.
        &MainBranchOid(source_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        true,
    )?;

    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "move")?;
    let result = move_subtree(
        out,
        err,
        git_executable,
        &repo,
        &graph,
        event_tx_id,
        source_oid,
        dest_oid,
        force_on_disk,
    )?;
    Ok(result)
}
