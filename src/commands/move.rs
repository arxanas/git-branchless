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
use cursive::utils::markup::StyledString;
use fn_error_context::context;
use indicatif::{ProgressBar, ProgressStyle};

use crate::core::eventlog::EventTransactionId;
use crate::core::eventlog::BRANCHLESS_TRANSACTION_ID_ENV_VAR;
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::formatting::printable_styled_string;
use crate::core::formatting::Glyphs;
use crate::core::graph::find_path_to_merge_base;
use crate::core::graph::{make_graph, BranchOids, CommitGraph, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::core::metadata::render_commit_metadata;
use crate::core::metadata::CommitMessageProvider;
use crate::core::metadata::CommitOidProvider;
use crate::util::get_main_branch_oid;
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

#[derive(Debug)]
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
    merge_base_db: &MergeBaseDb,
    graph: &CommitGraph,
    main_branch_oid: &MainBranchOid,
    source_oid: git2::Oid,
) -> anyhow::Result<Vec<RebaseCommand>> {
    let label_name = make_label_name(&repo, "onto".to_string())?;
    let (source_oid, mut rebase_plan) = {
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
            let rebase_plan: Vec<RebaseCommand> = path
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
            (*main_branch_oid, rebase_plan)
        } else {
            (source_oid, Vec::new())
        }
    };
    rebase_plan.push(RebaseCommand::Label {
        label_name: label_name.clone(),
    });
    let rebase_plan = plan_rebase_current(repo, graph, source_oid, &label_name, rebase_plan)?;
    Ok(rebase_plan)
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
    glyphs: &Glyphs,
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
    git_executable: &GitExecutable,
    repo: &git2::Repository,
    rebase_plan: &[RebaseCommand],
    source: git2::Oid,
    dest: git2::Oid,
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

#[context("Moving subtree from {} to {}", source.to_string(), dest.to_string())]
fn move_subtree(
    glyphs: &Glyphs,
    git_executable: &GitExecutable,
    repo: &git2::Repository,
    merge_base_db: &MergeBaseDb,
    graph: &CommitGraph,
    main_branch_oid: &MainBranchOid,
    event_tx_id: EventTransactionId,
    source: git2::Oid,
    dest: git2::Oid,
    force_on_disk: bool,
) -> anyhow::Result<isize> {
    // Make the rebase plan here so that it doesn't potentially fail after the
    // rebase has been initialized.
    let rebase_plan = make_rebase_plan(repo, merge_base_db, graph, main_branch_oid, source)?;

    if !force_on_disk {
        println!("Attempting rebase in-memory...");
        match rebase_in_memory(glyphs, &repo, &rebase_plan, dest)? {
            RebaseInMemoryResult::Succeeded { rewritten_oids } => {
                post_rebase_in_memory(&repo, &rewritten_oids, event_tx_id)?;
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
        source,
        dest,
        event_tx_id,
    )?;
    Ok(result)
}

/// Move a subtree from one place to another.
pub fn r#move(
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
            println!("Commit not found: {}", commit);
            return Ok(1);
        }
    };

    let main_branch_oid = get_main_branch_oid(&repo)?;
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
        &HeadOid(Some(source_oid)),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        true,
    )?;

    let glyphs = Glyphs::detect();
    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "move")?;
    let result = move_subtree(
        &glyphs,
        git_executable,
        &repo,
        &merge_base_db,
        &graph,
        &MainBranchOid(main_branch_oid),
        event_tx_id,
        source_oid,
        dest_oid,
        force_on_disk,
    )?;
    Ok(result)
}
