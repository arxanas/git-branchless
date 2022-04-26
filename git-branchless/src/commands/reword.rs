//! Update commit messages

use lib::core::check_out::CheckOutCommitOptions;
use lib::core::repo_ext::RepoExt;
use rayon::ThreadPoolBuilder;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::Write;
use std::time::SystemTime;

use dialoguer::Editor;
use eden_dag::DagAlgorithm;
use eyre::Context;
use tracing::{instrument, warn};

use lib::core::config::{
    get_comment_char, get_commit_template, get_editor, get_restack_preserve_timestamps,
};
use lib::core::dag::{resolve_commits, CommitSet, Dag, ResolveCommitsResult};
use lib::core::effects::Effects;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::{printable_styled_string, Glyphs, Pluralize};
use lib::core::node_descriptors::{render_node_descriptors, CommitOidDescriptor, NodeObject};
use lib::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanOptions, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    RebasePlanBuilder, RepoResource,
};
use lib::git::{message_prettify, Commit, GitRunInfo, MaybeZeroOid, NonZeroOid, Repo};

/// The commit message(s) provided by the user.
#[derive(Debug)]
pub enum InitialCommitMessages {
    /// The user wants to start with an empty (or template) message.
    Discard,

    /// The user provided explicit messages.
    Messages(Vec<String>),
}

/// Reword a commit and restack it's descendants.
#[instrument]
pub fn reword(
    effects: &Effects,
    hashes: Vec<String>,
    messages: InitialCommitMessages,
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

    let edit_message_fn = |message: &str| {
        let mut editor = Editor::new();
        let editor = match get_editor(&repo)? {
            Some(prog) => editor.executable(prog),
            None => &mut editor,
        };
        let result = editor
            .require_save(false)
            .edit(message)?
            .expect("`Editor::edit` should not return `None` when `require_save` is `false`");
        Ok(result)
    };

    let messages = match prepare_messages(&repo, messages, &commits, edit_message_fn)? {
        PrepareMessagesResult::Succeeded { messages } => messages,
        PrepareMessagesResult::IdenticalMessage => {
            writeln!(
                effects.get_output_stream(),
                "Aborting. The message wasn't edited; nothing to do."
            )?;
            return Ok(1);
        }
        PrepareMessagesResult::EmptyMessage => {
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
    let result = execute_rebase_plan(
        effects,
        git_run_info,
        &repo,
        &event_log_db,
        &rebase_plan,
        &execute_options,
    )?;

    let exit_code = match result {
        ExecuteRebasePlanResult::Succeeded {
            rewritten_oids: Some(rewritten_oids),
        } => {
            render_status_report(&repo, effects, &commits, &rewritten_oids)?;
            0
        }
        ExecuteRebasePlanResult::Succeeded {
            rewritten_oids: None,
        } => {
            writeln!(
                effects.get_error_stream(),
                "BUG: Succeeded rewording commits via on-disk rebase? But reword should be rebasing in-memory!"
            )?;
            1
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
#[derive(Debug)]
enum PrepareMessagesResult {
    /// The reworded message was empty.
    EmptyMessage,

    /// The reworded message matches the original message.
    IdenticalMessage,

    /// The reworded message was built successfully.
    Succeeded {
        /// The reworded messages for each commit.
        messages: HashMap<NonZeroOid, String>,
    },
}

/// Prepares the message(s) that will be used for rewording. These are mapped from each commit's
/// NonZeroOid to the relevant message.
#[instrument(skip(edit_message_fn))]
fn prepare_messages(
    repo: &Repo,
    messages: InitialCommitMessages,
    commits: &[Commit],
    edit_message_fn: impl Fn(&str) -> eyre::Result<String>,
) -> eyre::Result<PrepareMessagesResult> {
    let comment_char = get_comment_char(repo)?;

    let (message, load_editor, discard_messages) = match messages {
        InitialCommitMessages::Discard => {
            (get_commit_template(repo)?.unwrap_or_default(), true, true)
        }
        InitialCommitMessages::Messages(ref messages) => {
            let message = messages.clone().join("\n\n");
            let message = message.trim();
            (message.to_string(), message.is_empty(), false)
        }
    };

    if !load_editor {
        let message = message_prettify(message.as_str(), None)?;

        if message.trim().is_empty() {
            return Ok(PrepareMessagesResult::EmptyMessage);
        }

        let messages = commits
            .iter()
            .map(|commit| (commit.get_oid(), message.clone()))
            .collect();

        return Ok(PrepareMessagesResult::Succeeded { messages });
    };

    let mut message = if discard_messages {
        // TODO how should this work w/ bulk editing & --discard? just repeat the template, or perhaps show
        // template w/ original message commented out?
        message
    } else {
        // TODO what order are they listed in? (dag.sort()?)
        commits
            .iter()
            .map(
                |commit| match (commit.get_short_oid(), commit.get_message_raw()) {
                    (Ok(oid), Ok(msg)) => (oid, msg),
                    (Err(_), Ok(_)) => {
                        panic!("Could not get short OID for commit: {:?}", commit.get_oid())
                    }
                    (Ok(_), Err(_)) => {
                        panic!(
                            "Could not get raw message for commit: {:?}",
                            commit.get_oid()
                        )
                    }
                    (Err(_), Err(_)) => panic!(
                        "Could not get short OID or raw message for commit: {:?}",
                        commit.get_oid()
                    ),
                },
            )
            .map(|(hash, msg)| match msg.into_string() {
                Ok(msg) => (hash, msg),
                Err(_) => panic!("Could not decode original message for commit: {}", hash),
            })
            .map(|(hash, msg)| format!("++ reword {}\n{}", hash, msg))
            .collect::<Vec<String>>()
            .join("\n")
    };

    message.push_str(
        format!(
            "{}\
                {} Rewording: Please enter the commit {} to apply to {}. Lines\n\
                {} starting with '{}' will be ignored, and an empty message aborts rewording.",
            if message.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            },
            comment_char,
            match commits.len() {
                1 => "message",
                _ => "messages",
            },
            Pluralize {
                determiner: Some(("this", "these")),
                amount: commits.len(),
                unit: ("commit", "commits"),
            },
            comment_char,
            comment_char,
        )
        .as_str(),
    );

    let edited_message = edit_message_fn(&message)?;
    if edited_message == message {
        return Ok(PrepareMessagesResult::IdenticalMessage);
    }

    let message = message_prettify(edited_message.as_str(), Some(comment_char))?;
    if message.trim().is_empty() {
        return Ok(PrepareMessagesResult::EmptyMessage);
    }

    let messages = parse_bulk_edit_message(message, commits, comment_char)?;

    Ok(PrepareMessagesResult::Succeeded { messages })
}

fn parse_bulk_edit_message(
    message: String,
    commits: &[Commit],
    comment_char: char,
) -> eyre::Result<HashMap<NonZeroOid, String>> {
    let mut commits_oids = HashMap::new();
    for commit in commits.iter() {
        commits_oids.insert(commit.get_short_oid()?, commit.get_oid());
    }

    // split the bulk message into (hash, msg) tuples
    let messages = message
        .split("++ reword")
        .filter_map(|msg| msg.split_once('\n'))
        .map(|(hash, msg)| (commits_oids.get(hash.trim()), msg))
        .filter(|(oid, _)| oid.is_some())
        .map(|(oid, msg)| {
            (
                *oid.unwrap(),
                message_prettify(msg, Some(comment_char)).expect("TODO"),
            )
        })
        .collect();

    Ok(messages)
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

#[cfg(test)]
mod tests {
    use lib::testing::make_git;

    use super::*;

    #[test]
    fn test_reword_uses_commit_template() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        let repo = git.get_repo()?;

        let head_oid = git.commit_file("test1", 1)?;
        let head_commit = repo.find_commit_or_fail(head_oid)?;

        {
            let result = prepare_messages(
                &repo,
                InitialCommitMessages::Discard,
                &[head_commit.clone()],
                |message| {
                    insta::assert_snapshot!(message.trim(), @r###"
                    # Rewording: Please enter the commit message to apply to this 1 commit. Lines
                    # starting with '#' will be ignored, and an empty message aborts rewording.
                    "###);
                    Ok(message.to_string())
                },
            )?;
            insta::assert_debug_snapshot!(result, @"IdenticalMessage");
        }

        git.run(&["config", "commit.template", "template.txt"])?;
        git.write_file(
            "template",
            "\
This is a template!
",
        )?;

        {
            let result = prepare_messages(
                &repo,
                InitialCommitMessages::Discard,
                &[head_commit],
                |message| {
                    insta::assert_snapshot!(message.trim(), @r###"
                    This is a template!

                    # Rewording: Please enter the commit message to apply to this 1 commit. Lines
                    # starting with '#' will be ignored, and an empty message aborts rewording.
                    "###);
                    Ok(message.to_string())
                },
            )?;
            insta::assert_debug_snapshot!(result, @"IdenticalMessage");
        }

        Ok(())
    }

    #[test]
    fn test_reword_builds_multi_commit_messages() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        let repo = git.get_repo()?;

        let test1_oid = git.commit_file("test1", 1)?;
        let test2_oid = git.commit_file("test2", 2)?;
        let test1_commit = repo.find_commit_or_fail(test1_oid)?;
        let test2_commit = repo.find_commit_or_fail(test2_oid)?;

        {
            let result = prepare_messages(
                &repo,
                InitialCommitMessages::Messages([].to_vec()),
                &[test1_commit.clone(), test2_commit.clone()],
                |message| {
                    insta::assert_snapshot!(message.trim(), @r###"
                    ++ reword 62fc20d
                    create test1.txt

                    ++ reword 96d1c37
                    create test2.txt

                    # Rewording: Please enter the commit messages to apply to these 2 commits. Lines
                    # starting with '#' will be ignored, and an empty message aborts rewording.
                    "###);
                    Ok(message.to_string())
                },
            )?;
            insta::assert_debug_snapshot!(result, @"IdenticalMessage");
        }

        Ok(())
    }

    #[test]
    fn test_parse_bulk_edit_message() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        let repo = git.get_repo()?;

        let test1_oid = git.commit_file("test1", 1)?;
        let test2_oid = git.commit_file("test2", 2)?;
        let test1_commit = repo.find_commit_or_fail(test1_oid)?;
        let test2_commit = repo.find_commit_or_fail(test2_oid)?;

        {
            let result = parse_bulk_edit_message(
                String::from(
                    "++ reword 62fc20d\n\
                create test1.txt\n\
                \n\
                ++ reword 96d1c37\n\
                create test2.txt\n",
                ),
                &[test1_commit.clone(), test2_commit.clone()],
                '#',
            )?;

            assert_eq!(2, result.len());
            match result.get(&test1_oid) {
                Some(msg) => assert_eq!("create test1.txt\n", msg.as_str()),
                None => panic!("Parsed messages did not contain {:?}", test1_oid),
            };
            match result.get(&test2_oid) {
                Some(msg) => assert_eq!("create test2.txt\n", msg.as_str()),
                None => panic!("Parsed messages did not contain {:?}", test2_oid),
            };
        };

        Ok(())
    }
}
