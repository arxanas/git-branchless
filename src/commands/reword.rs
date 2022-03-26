//! Update commit messages

use rayon::ThreadPoolBuilder;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::Write;
use std::time::SystemTime;

use dialoguer::Editor;
use eden_dag::DagAlgorithm;
use eyre::Context;
use tracing::{instrument, warn};

use crate::core::config::{get_comment_char, get_restack_preserve_timestamps};
use crate::core::dag::{resolve_commits, CommitSet, Dag, ResolveCommitsResult};
use crate::core::effects::Effects;
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::formatting::{printable_styled_string, Glyphs};
use crate::core::node_descriptors::{render_node_descriptors, CommitOidDescriptor, NodeObject};
use crate::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanOptions, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    RebasePlanBuilder, RepoResource,
};
use crate::git::{CheckOutCommitOptions, Commit, GitRunInfo, MaybeZeroOid, NonZeroOid, Repo};

/// Reword a commit and restack it's descendants.
#[instrument]
pub fn reword(
    effects: &Effects,
    hashes: Vec<String>,
    messages: Vec<String>,
    git_run_info: &GitRunInfo,
) -> eyre::Result<isize> {
    let repo = Repo::from_current_dir()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let commits = match resolve_commits_from_hashes(&repo, &mut dag, effects, hashes)? {
        Some(commits) => commits,
        None => return Ok(1),
    };

    let messages = match build_messages(&messages, &commits, get_comment_char(&repo)?)? {
        BuildRewordMessageResult::Succeeded { messages } => messages,
        BuildRewordMessageResult::IdenticalMessage => {
            writeln!(
                effects.get_output_stream(),
                "Aborting. The message wasn't edited; nothing to do."
            )?;
            return Ok(1);
        }
        BuildRewordMessageResult::EmptyMessage => {
            writeln!(
                effects.get_error_stream(),
                "Aborting reword due to empty commit message."
            )?;
            return Ok(1);
        }
    };

    let subtree_roots = find_subtree_roots(&repo, &dag, &commits)?;

    let rebase_plan = {
        let pool = ThreadPoolBuilder::new().build()?;
        let repo_pool = RepoResource::new_pool(&repo)?;
        let mut builder = RebasePlanBuilder::new(&dag);

        for root_commit in subtree_roots {
            let only_parent_id = root_commit.get_only_parent().map(|parent| parent.get_oid());
            let only_parent_id = match only_parent_id {
                Some(only_parent_id) => only_parent_id,
                None => {
                    writeln!(
                        effects.get_error_stream(),
                        "Refusing to reword commit {}, which has {} parents.\n\
                        Rewording is only supported for commits with 1 parent.\n\
                        Aborting.",
                        root_commit.get_oid(),
                        root_commit.get_parents().len(),
                    )?;
                    return Ok(1);
                }
            };
            builder.move_subtree(root_commit.get_oid(), only_parent_id)?;
        }

        for commit in commits.iter() {
            let message = messages.get(&commit.get_oid()).unwrap();
            // This looks funny, but just means "leave everything but the message as is"
            let replacement_oid =
                commit.amend_commit(None, None, None, Some(message.as_str()), None)?;
            builder.replace_commit(commit.get_oid(), replacement_oid)?;
        }

        match builder.build(
            effects,
            &pool,
            &repo_pool,
            &BuildRebasePlanOptions {
                dump_rebase_constraints: false,
                dump_rebase_plan: false,
                detect_duplicate_commits_via_patch_id: false,
            },
        )? {
            Ok(Some(rebase_plan)) => rebase_plan,
            Ok(None) => {
                eyre::bail!(
                    "BUG: rebase plan indicates nothing to do, but rewording should always do something."
                );
            }
            Err(err) => {
                err.describe(effects, &repo)?;
                return Ok(1);
            }
        }
    };

    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "reword")?;
    let execute_options = ExecuteRebasePlanOptions {
        now,
        event_tx_id,
        preserve_timestamps: get_restack_preserve_timestamps(&repo)?,
        force_in_memory: true,
        force_on_disk: false,
        resolve_merge_conflicts: false,
        check_out_commit_options: CheckOutCommitOptions {
            additional_args: Default::default(),
            render_smartlog: false,
        },
    };
    let result = execute_rebase_plan(effects, git_run_info, &repo, &rebase_plan, &execute_options)?;

    let exit_code = match result {
        ExecuteRebasePlanResult::Succeeded { rewritten_oids } => {
            render_status_report(&repo, effects, &commits, &rewritten_oids)?;
            0
        }
        ExecuteRebasePlanResult::DeclinedToMerge { merge_conflict: _ } => {
            writeln!(
                effects.get_error_stream(),
                "BUG: Merge conflict detected, but rewording shouldn't cause any conflicts."
            )?;
            1
        }
        ExecuteRebasePlanResult::Failed { exit_code } => exit_code,
    };

    Ok(exit_code)
}

/// Turn a list of ref-ish strings into a list of Commits.
fn resolve_commits_from_hashes<'repo>(
    repo: &'repo Repo,
    dag: &mut Dag,
    effects: &Effects,
    hashes: Vec<String>,
) -> eyre::Result<Option<Vec<Commit<'repo>>>> {
    let hashes = if hashes.is_empty() {
        vec!["HEAD".to_string()]
    } else {
        hashes
    };

    let commits = resolve_commits(effects, repo, dag, hashes)?;
    let commits = match commits {
        ResolveCommitsResult::Ok { commits } => commits,
        ResolveCommitsResult::CommitNotFound { commit: hash } => {
            writeln!(effects.get_output_stream(), "Commit not found: {}", hash)?;
            return Ok(None);
        }
    };
    Ok(Some(commits))
}

/// The result of building the reword message.
#[must_use]
pub enum BuildRewordMessageResult {
    /// The reworded message was built successfully.
    Succeeded {
        /// The reworded messages for each commit.
        messages: HashMap<NonZeroOid, String>,
    },

    /// The reworded message matches the original message.
    IdenticalMessage,

    /// The reworded message was empty.
    EmptyMessage,
}

/// Builds the message(s) that will be used for rewording. These are mapped from each commit's
/// NonZeroOid to the relevant message.
#[instrument]
fn build_messages(
    messages: &[String],
    commits: &[Commit],
    comment_char: Option<u8>,
) -> eyre::Result<BuildRewordMessageResult> {
    let message = messages.join("\n\n").trim().to_string();

    let (mut message, load_editor) = if message.is_empty() {
        let mut message_for_editor = match commits {
            [commit] => match commit.get_message_raw()?.into_string() {
                Ok(msg) => msg,
                Err(_) => eyre::bail!(
                    "Error decoding original message for commit: {:?}.\n\
                    Aborting.",
                    commit.get_oid()
                ),
            },
            _ => {
                // TODO(bulk edit) build a bulk edit message for multiple commits
                format!(
                    "{} Enter a commit message to apply to {} commits",
                    comment_char.unwrap(),
                    commits.len()
                )
            }
        };
        (message_for_editor, true)
    } else {
        (message, false)
    };

    let message = if load_editor {
        // Editor::edit() normally requires that the file being editted is saved before the editor
        // is closed. If it's not, it will return None; and in all other cases, it will return
        // Some(message). We don't care if the file has been saved, only if it hasn't changed, so
        // here we call `require_save(false)` and just ignore the None case.
        let edited_message = Editor::new()
            .require_save(false)
            .edit(message.as_str())?
            .unwrap();
        if edited_message == message {
            return Ok(BuildRewordMessageResult::IdenticalMessage);
        }
        edited_message
    } else {
        message
    };

    // clean up the commit message: remove comment lines, finish w/ a newline, etc
    // TODO ask if this should be here or if it should be moved to the git module per project wiki
    let message = git2::message_prettify(message, comment_char)?;

    if message.trim().is_empty() {
        return Ok(BuildRewordMessageResult::EmptyMessage);
    }

    // TODO(bulk edit) process message if it looks like a bulk edit message
    // - break it apart in separate messages
    // - parse the marker lines into hashes
    // - map hashes into NonZeroOid
    // - create a map with OIDs as keys and relevant messages as values
    // - confirm that we have edits for the right set of commits
    //     - just those spec'd on the CLI, no more no less
    //     - no duplicates

    let messages: HashMap<NonZeroOid, String> = commits
        .iter()
        .map(|commit| (commit.get_oid(), message.clone()))
        .collect();

    Ok(BuildRewordMessageResult::Succeeded { messages })
}

/// Return the root commits for given a list of commits. This is the list of commits that have *no*
/// ancestors also in the list. The idea is to find the minimum number of subtrees that much be
/// rebased to include all of our rewording.
fn find_subtree_roots<'repo>(
    repo: &'repo Repo,
    dag: &Dag,
    commits: &[Commit],
) -> eyre::Result<Vec<Commit<'repo>>> {
    let commits: CommitSet = commits.iter().map(|commit| commit.get_oid()).collect();

    // Find the vertices representing the roots of this set of commits
    let subtree_roots = dag
        .query()
        .roots(commits)
        .wrap_err("Computing subtree roots")?;

    // convert the vertices back into actual Commits
    let root_commits = subtree_roots
        .iter()?
        .filter_map(|vertex| NonZeroOid::try_from(vertex.ok()?).ok())
        .filter_map(|oid| repo.find_commit(oid).ok()?)
        .collect();

    Ok(root_commits)
}

/// Print a basic status report of what commits were reworded.
fn render_status_report(
    repo: &Repo,
    effects: &Effects,
    commits: &[Commit],
    rewritten_oids: &HashMap<NonZeroOid, MaybeZeroOid>,
) -> eyre::Result<()> {
    let glyphs = Glyphs::detect();
    let num_commits = commits.len();
    for original_commit in commits {
        let replacement_oid = match rewritten_oids.get(&original_commit.get_oid()) {
            Some(MaybeZeroOid::NonZero(new_oid)) => new_oid,
            Some(MaybeZeroOid::Zero) => {
                warn!(
                    "Encountered ZeroOid after success rewriting commit {}",
                    original_commit.get_oid()
                );
                continue;
            }
            None => {
                writeln!(
                    effects.get_error_stream(),
                    "Warning: Could not find rewritten commit for {}",
                    original_commit.get_oid(),
                )?;
                continue;
            }
        };
        let replacement_commit = repo.find_commit(*replacement_oid)?.unwrap();
        writeln!(
            effects.get_output_stream(),
            "Reworded commit {} as {}",
            printable_styled_string(
                &glyphs,
                // Commit doesn't offer `friendly_describe_oid`, so we'll do it ourselves
                render_node_descriptors(
                    &glyphs,
                    &NodeObject::Commit {
                        commit: original_commit.clone(),
                    },
                    &mut [&mut CommitOidDescriptor::new(true)?],
                )?
            )?,
            printable_styled_string(&glyphs, replacement_commit.friendly_describe(&glyphs)?)?
        )?;
    }

    if num_commits != 1 {
        writeln!(
            effects.get_output_stream(),
            "Reworded {} commits with same message. If this was unintentional, run: git undo",
            num_commits,
        )?;
    }

    Ok(())
}
