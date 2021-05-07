//! Move commits and subtrees from one place to another.
//!
//! Under the hood, this makes use of Git's advanced rebase functionality, which
//! is also used to preserve merge commits using the `--rebase-merges` option.

use std::io::Write;
use std::time::SystemTime;

use git2::RebaseOptions;

use crate::core::eventlog::EventTransactionId;
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::graph::{make_graph, BranchOids, CommitGraph, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::testing::get_git_executable;
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

fn move_subtree(
    out: &mut impl Write,
    err: &mut impl Write,
    git_executable: &GitExecutable,
    repo: &git2::Repository,
    graph: &CommitGraph,
    event_tx_id: EventTransactionId,
    source: git2::Oid,
    dest: git2::Oid,
) -> anyhow::Result<isize> {
    // Make the rebase plan here so that it doesn't potentially fail after the
    // rebase has been initialized.
    let rebase_plan = make_rebase_plan(&repo, &graph, source)?;

    // Attempt to initialize a new rebase. However, `git2` doesn't support the
    // commands we need (`label` and `reset`), so we won't be using it for the
    // actual rebase process.
    let mut rebase_options = RebaseOptions::default();
    let _rebase: git2::Rebase = repo.rebase(
        None,
        // TODO: if the target was a branch, do we need to use an annotated
        // commit which was instantiated from the branch?
        Some(&repo.find_annotated_commit(source)?),
        Some(&repo.find_annotated_commit(dest)?),
        Some(&mut rebase_options),
    )?;

    let todo_file = repo.path().join("rebase-merge").join("git-rebase-todo");
    std::fs::write(
        todo_file,
        rebase_plan
            .iter()
            .map(|command| format!("{}\n", command.to_string()))
            .collect::<String>(),
    )?;
    let end_file = repo.path().join("rebase-merge").join("end");
    std::fs::write(end_file, format!("{}\n", rebase_plan.len()))?;

    let result = run_git(
        out,
        err,
        &git_executable,
        Some(event_tx_id),
        &["rebase", "--continue"],
    )?;
    Ok(result)
}

/// Move a subtree from one place to another.
pub fn r#move(
    out: &mut impl Write,
    err: &mut impl Write,
    source: Option<String>,
    dest: Option<String>,
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
    let git_executable = get_git_executable()?;
    let result = move_subtree(
        out,
        err,
        &GitExecutable(&git_executable),
        &repo,
        &graph,
        event_tx_id,
        source_oid,
        dest_oid,
    )?;
    Ok(result)
}
