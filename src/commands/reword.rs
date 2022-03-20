//! Update commit messages

use rayon::ThreadPoolBuilder;
use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use std::fmt::Write;
use std::time::SystemTime;

use dialoguer::{Confirm, Editor};
use eden_dag::DagAlgorithm;
use tracing::instrument;

use crate::core::config::get_restack_preserve_timestamps;
use crate::core::dag::{resolve_commits, CommitSet, Dag, ResolveCommitsResult};
use crate::core::effects::{Effects, OperationType};
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanError, BuildRebasePlanOptions, ExecuteRebasePlanOptions,
    ExecuteRebasePlanResult, RebasePlan, RebasePlanBuilder, RepoResource,
};
use crate::git::{Commit, GitRunInfo, NonZeroOid, Repo};
use crate::opts::RebaseOptions;

/// Reword a commit and restack it's descendants.
#[instrument]
pub fn reword(
    effects: &Effects,
    force: bool,
    hashes: Vec<String>,
    messages: Vec<String>,
    git_run_info: &GitRunInfo,
    rebase_options: &RebaseOptions,
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

    let messages = match build_messages(&messages, &commits)? {
        BuildRewordMessageResult::Succeeded { messages } => messages,
        BuildRewordMessageResult::IdenticalMessage => {
            println!("The message wasn't edited; nothing to do. Stopping.");
            return Ok(1);
        }
        BuildRewordMessageResult::EmptyMessage => {
            eprintln!("Error: the reworded message was empty. Stopping.");
            return Ok(1);
        }
        BuildRewordMessageResult::Failed { message } => {
            if message != None {
                eprintln!("{}", message.unwrap());
            }
            eprintln!("Error: problem processing commit message. Nothing reworded.");
            return Ok(1);
        }
    };

    if commits.len() != 1
        && !force
        && !Confirm::new()
            .with_prompt(format!(
                "Warning: attempting to apply the same message to {} commits. Continue?",
                commits.len()
            ))
            .default(false)
            .interact()?
    {
        println!("Ok. Nothing reworded.");
        return Ok(1);
    }

    let subtree_roots = find_subtree_roots(&repo, &dag, &commits)?;

    // TODO instead of calling into `r#move` shoud we just be building the rebase plan ourselves?
    // TODO look at `sync` to see how it handles return values for multiple subtree moves
    // TODO what if there is no parent?
    // TODO what if there are multiple parents?
    let RebaseOptions {
        force_in_memory,
        force_on_disk,
        detect_duplicate_commits_via_patch_id,
        resolve_merge_conflicts,
        dump_rebase_constraints,
        dump_rebase_plan,
    } = *rebase_options;

    let subtree_roots_and_plans: Vec<(Commit, Option<RebasePlan>)> = {
        let pool = ThreadPoolBuilder::new().build()?;
        let repo_pool = RepoResource::new_pool(&repo)?;
        let builder = RebasePlanBuilder::new(&dag);

        let subtree_roots_and_plans =
            subtree_roots
                .into_iter()
                .map(
                    |root_commit| -> eyre::Result<
                        Result<(Commit, Option<RebasePlan>), BuildRebasePlanError>,
                    > {
                        // Keep access to the same underlying caches by cloning the same instance of the
                        // builder.
                        let mut builder = builder.clone();

                        let rebase_plan = {
                            let only_parent_id =
                                root_commit.get_only_parent().map(|parent| parent.get_oid());
                            if only_parent_id == None {
                                // TODO multiple parents!
                            }
                            builder.move_subtree(root_commit.get_oid(), only_parent_id.unwrap())?;
                            builder.reword_commits(&messages)?;
                            builder.build(
                                effects,
                                &pool,
                                &repo_pool,
                                &BuildRebasePlanOptions {
                                    dump_rebase_constraints,
                                    dump_rebase_plan,
                                    detect_duplicate_commits_via_patch_id,
                                },
                            )?
                        };

                        Ok(rebase_plan.map(|rebase_plan| (root_commit, rebase_plan)))
                    },
                )
                .collect::<eyre::Result<Vec<_>>>()?
                .into_iter()
                .collect::<Result<Vec<_>, BuildRebasePlanError>>();

        match subtree_roots_and_plans {
            Ok(subtree_roots_and_plans) => subtree_roots_and_plans,
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
        force_in_memory,
        force_on_disk,
        resolve_merge_conflicts,
        check_out_commit_options: Default::default(),
    };

    let (_success_commits, _merge_conflict_commits, _skipped_commits) = {
        let mut success_commits: Vec<Commit> = Vec::new();
        let mut merge_conflict_commits: Vec<Commit> = Vec::new();
        let mut skipped_commits: Vec<Commit> = Vec::new();

        let (effects, progress) = effects.start_operation(OperationType::RebaseCommits);
        progress.notify_progress(0, subtree_roots_and_plans.len());

        for (root_commit, rebase_plan) in subtree_roots_and_plans {
            let rebase_plan = match rebase_plan {
                Some(rebase_plan) => rebase_plan,
                None => {
                    // Nothing to do ... but why? Bug?
                    skipped_commits.push(root_commit);
                    continue;
                }
            };

            let result = execute_rebase_plan(
                &effects,
                git_run_info,
                &repo,
                &rebase_plan,
                &execute_options,
            )?;
            progress.notify_progress_inc(1);

            match result {
                ExecuteRebasePlanResult::Succeeded => {
                    success_commits.push(root_commit);
                }
                ExecuteRebasePlanResult::DeclinedToMerge { merge_conflict: _ } => {
                    // FIXME not sure this is even possible here...
                    merge_conflict_commits.push(root_commit);
                }
                ExecuteRebasePlanResult::Failed { exit_code } => {
                    return Ok(exit_code);
                }
            }
        }

        (success_commits, merge_conflict_commits, skipped_commits)
    };

    // FIXME how much of this is relevant? User asked to reword commits, but we've rebased subtrees
    // If we really want to show a relevant message perhaps it should be something like
    // "executed X rebases to reword Y commits"?
    //
    // for success_commit in success_commits {
    //     writeln!(
    //         effects.get_output_stream(),
    //         "{}",
    //         printable_styled_string(
    //             &glyphs,
    //             StyledStringBuilder::new()
    //                 .append_plain("Synced ")
    //                 .append(success_commit.friendly_describe(&glyphs)?)
    //                 .build()
    //         )?
    //     )?;
    // }
    //
    // for merge_conflict_commit in merge_conflict_commits {
    //     writeln!(
    //         effects.get_output_stream(),
    //         "{}",
    //         printable_styled_string(
    //             &glyphs,
    //             StyledStringBuilder::new()
    //                 .append_plain("Merge conflict for ")
    //                 .append(merge_conflict_commit.friendly_describe(&glyphs)?)
    //                 .build()
    //         )?
    //     )?;
    // }
    //
    // for skipped_commit in skipped_commits {
    //     writeln!(
    //         effects.get_output_stream(),
    //         "Not moving up-to-date stack at {}",
    //         printable_styled_string(&glyphs, skipped_commit.friendly_describe(&glyphs)?)?
    //     )?;
    // }

    Ok(0)
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

    /// Misc failure.
    Failed {
        /// The failue message, if any.
        message: Option<String>,
    },
}

/// Builds the message(s) that will be used for rewording. These are mapped from each commit's
/// NonZeroOid to the relevant message.
fn build_messages(
    messages: &[String],
    commits: &[Commit],
) -> eyre::Result<BuildRewordMessageResult> {
    let message = messages.join("\n\n").trim().to_string();

    // TODO(maybe?) stdin?

    let (message, load_editor) = if message.is_empty() {
        let message_for_editor = match commits.len() {
            1 => {
                // FIXME lots of error checking to do
                let commit = commits.first().unwrap();
                let msg = commit.get_message_raw()?;
                let msg = msg.into_string();
                match msg {
                    Ok(msg) => msg,
                    _ => {
                        return Ok(BuildRewordMessageResult::Failed {
                            message: Some(String::from(
                                "Reword: Error decoding original commit message!",
                            )),
                        })
                    }
                }
            }
            len => {
                // TODO build a bulk edit message for multiple commits
                format!("Enter a commit message to apply to {} commits", len)
            }
        };
        (message_for_editor, true)
    } else {
        (message, false)
    };

    let message = if load_editor {
        // Editor::edit will only return None if saving is required; b/c we've disabled the save
        // requirement, we don't need to worry about it.
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

    // TODO process the message: remove comment lines, trim, etc

    if message.is_empty() {
        return Ok(BuildRewordMessageResult::EmptyMessage);
    }

    // TODO if the message appears to be a "bulk edit" message, break it apart
    // TODO FIXME what if the bulk edit message doesn't include messages for != `commits.len`
    // TODO FIXME what if the bulk edit message includes messages for commits not in `commits`

    let messages: HashMap<NonZeroOid, String> = {
        let mut m = HashMap::new();
        for commit in commits.iter() {
            m.insert(commit.get_oid(), message.clone());
        }
        m
    };

    Ok(BuildRewordMessageResult::Succeeded { messages })
}

/// Given a set of commits, remove all commits that have ancestors also in the set. In other words,
/// leave behind only the commits that have *no* ancestors in the set. The idea is to find the
/// minimum number of subtrees that much be rebased to include all of our rewording. This is
/// similar to "greatest common ancestor" except that this is more like "greatest common ancestors
/// for each subtree included in this set".
///
/// Example commit graph:
/// a - b - c - d
///  \   \- e - f
///   \---- g - h
///
/// For example, given the above graph, the subtree roots should be this:
/// - *query*      => *result*
/// - `c, d`       => `c`
/// - `d, f`       => `d, f`
/// - `b, d, e`    => `b`
/// - `b, c, e, g` => `b, g`
fn find_subtree_roots<'repo>(
    repo: &'repo Repo,
    dag: &Dag,
    commits: &[Commit],
) -> eyre::Result<Vec<Commit<'repo>>> {
    let mut subtree_roots: HashSet<NonZeroOid> = HashSet::new();
    let commits: CommitSet = commits
        .iter()
        .map(|commit| commit.get_oid())
        .rev()
        .collect();

    // create a working collection from our base collection of commits
    let mut working_set = commits.clone();

    // iterate over each commit in working collection
    let mut i = 0;
    while !working_set.is_empty()? {
        // get the first (aka "HEAD-iest") commit left in the set
        let current_commit = CommitSet::from_static_names(working_set.first()?);

        // get *all* ancestors of this commit
        let all_ancestors = dag.query().ancestors(current_commit)?;

        // which of these ancestors are relevant (ie are included in our base set of commits)
        let relevant_ancestors = commits.intersection(&all_ancestors);
        let relevant_ancestors = dag.query().sort(&relevant_ancestors)?;

        // get the last commit from these relevant ancestors; it is the root of the subtree that
        // contains current_commit (and convert it from a VertexName into a NonZeroOid)
        let last_ancestor = NonZeroOid::try_from(relevant_ancestors.last()?.unwrap());
        // TODO error check on the result of try_from
        subtree_roots.insert(last_ancestor?);

        // finally, remove these ancestors from our working set; we've already found their common
        // root
        working_set = working_set - relevant_ancestors;

        // now finally finally, just cover our behind in case I really screwed something up!
        i += 1;
        if i == 100 {
            println!("Bailing out after 100 loop iterations: this is either a REALLY big query or a bug.");
            break;
        }
    }

    // convert all of the NonZeroOid into actual Commits
    let root_commits: Vec<Commit> = {
        let mut commits = Vec::new();
        for commit_oid in subtree_roots.into_iter() {
            if let Some(commit) = repo.find_commit(commit_oid)? {
                commits.push(commit)
            }
        }
        commits
    };

    Ok(root_commits)
}
