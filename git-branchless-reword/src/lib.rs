//! Update commit messages

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

pub mod dialoguer_edit;

use lib::core::check_out::CheckOutCommitOptions;
use lib::core::repo_ext::RepoExt;
use lib::util::{ExitCode, EyreExitOr};
use rayon::ThreadPoolBuilder;
use std::collections::{HashMap, HashSet};

use std::fmt::Write;
use std::fs::File;
use std::time::SystemTime;

use bstr::{ByteSlice, ByteVec};
use chrono::Local;
use dialoguer_edit::Editor;

use eyre::Context;
use tracing::{instrument, warn};

use lib::core::config::{
    get_comment_char, get_commit_template, get_editor, get_restack_preserve_timestamps,
};
use lib::core::dag::{sorted_commit_set, union_all, CommitSet, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::{Glyphs, Pluralize};
use lib::core::node_descriptors::{render_node_descriptors, CommitOidDescriptor, NodeObject};
use lib::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanOptions, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    RebasePlanBuilder, RebasePlanPermissions, RepoResource,
};
use lib::git::{message_prettify, Commit, GitRunInfo, MaybeZeroOid, NonZeroOid, Repo};

use git_branchless_opts::{ResolveRevsetOptions, Revset, SignOptions};
use git_branchless_revset::resolve_commits;

/// The commit message(s) provided by the user.
#[derive(Debug)]
pub enum InitialCommitMessages {
    /// The user wants to start with an empty (or template) message.
    Discard,

    /// The user wants to fixup a commit.
    FixUp(Revset),

    /// The user provided explicit messages.
    Messages(Vec<String>),
}

/// Open the user's configured commit editor seeded with the provided message.
#[instrument]
pub fn edit_message(git_run_info: &GitRunInfo, repo: &Repo, message: &str) -> eyre::Result<String> {
    let (mut editor, editor_program) = match get_editor(git_run_info, repo)? {
        Some(editor_program) => {
            let mut editor = Editor::new();
            editor.executable(&editor_program);
            (editor, editor_program)
        }
        None => (Editor::new(), "<default>".into()),
    };
    if editor_program == ":" {
        // Special case in Git: treat `:` as a no-op editor.
        return Ok(message.to_string());
    }
    let result = editor
        .require_save(false)
        .edit(message)
        .with_context(|| format!("Invoking editor: '{}'", editor_program.to_string_lossy()))?
        .expect("`Editor::edit` should not return `None` when `require_save` is `false`");
    Ok(result)
}

/// Reword a commit and restack its descendants.
#[instrument]
pub fn reword(
    effects: &Effects,
    revsets: Vec<Revset>,
    resolve_revset_options: &ResolveRevsetOptions,
    messages: InitialCommitMessages,
    git_run_info: &GitRunInfo,
    force_rewrite_public_commits: bool,
    sign_options: SignOptions,
) -> EyreExitOr<()> {
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

    let commits = match resolve_commits_from_hashes(
        &repo,
        &mut dag,
        effects,
        revsets,
        resolve_revset_options,
    )? {
        Some(commits) => commits,
        None => return Ok(Err(ExitCode(1))),
    };
    let build_options = BuildRebasePlanOptions {
        force_rewrite_public_commits,
        dump_rebase_constraints: false,
        dump_rebase_plan: false,
        detect_duplicate_commits_via_patch_id: false,
    };
    let permissions = match RebasePlanPermissions::verify_rewrite_set(
        &dag,
        build_options,
        &commits.iter().map(|commit| commit.get_oid()).collect(),
    )? {
        Ok(permissions) => permissions,
        Err(err) => {
            err.describe(effects, &repo, &dag)?;
            return Ok(Err(ExitCode(1)));
        }
    };

    let messages = match messages {
        InitialCommitMessages::Discard | InitialCommitMessages::Messages(_) => messages,
        InitialCommitMessages::FixUp(revset) => {
            let commits_to_fixup = resolve_commits_from_hashes(
                &repo,
                &mut dag,
                effects,
                vec![revset.clone()],
                resolve_revset_options,
            )?
            .unwrap_or_default();
            let commit_to_fixup = match commits_to_fixup.as_slice() {
                [commit_to_fixup] => {
                    let commits: CommitSet = commits.iter().map(|c| c.get_oid()).collect();
                    if !dag.set_contains(
                        &dag.query_common_ancestors(commits)?,
                        commit_to_fixup.get_oid(),
                    )? {
                        writeln!(
                            effects.get_error_stream(),
                            "The commit supplied to --fixup must be an ancestor of all commits being reworded.\nAborting.",
                        )?;
                        return Ok(Err(ExitCode(1)));
                    }
                    commit_to_fixup
                }
                commits => {
                    writeln!(
                        effects.get_error_stream(),
                        "--fixup expects exactly 1 commit, but '{}' evaluated to {}.\nAborting.",
                        revset,
                        commits.len()
                    )?;
                    return Ok(Err(ExitCode(1)));
                }
            };
            let message = commit_to_fixup.get_summary()?.to_vec();
            let message = format!("fixup! {}", message.into_string_lossy());
            InitialCommitMessages::Messages(vec![message])
        }
    };

    let edit_message_fn = |message: &str| edit_message(git_run_info, &repo, message);

    let messages = match prepare_messages(&repo, messages, &commits, edit_message_fn)? {
        PrepareMessagesResult::Succeeded { messages } => messages,
        PrepareMessagesResult::IdenticalMessage => {
            writeln!(
                effects.get_output_stream(),
                "Aborting. The message was not edited; nothing to do."
            )?;
            return Ok(Err(ExitCode(1)));
        }
        PrepareMessagesResult::EmptyMessage => {
            writeln!(
                effects.get_error_stream(),
                "Aborting reword due to empty commit message."
            )?;
            return Ok(Err(ExitCode(1)));
        }
        PrepareMessagesResult::MismatchedCommits {
            mut duplicates,
            mut missing,
            mut unexpected,
        } => {
            writeln!(
                effects.get_error_stream(),
                "Aborting reword due to mismatched inputs."
            )?;
            if !missing.is_empty() {
                missing.sort_unstable();
                writeln!(
                    effects.get_error_stream(),
                    "{} specified on the command line, but not found in the edited message:\n{}",
                    Pluralize {
                        determiner: Some(("This", "These")),
                        amount: missing.len(),
                        unit: ("commit was", "commits were"),
                    },
                    missing.join(", ")
                )?;
            }
            if !unexpected.is_empty() {
                unexpected.sort_unstable();
                writeln!(
                    effects.get_error_stream(),
                    "{} found in the edited message, but {} not expected:\n{}",
                    Pluralize {
                        determiner: Some(("This", "These")),
                        amount: unexpected.len(),
                        unit: ("commit was", "commits were"),
                    },
                    match unexpected.len() {
                        1 => "was",
                        _ => "were",
                    },
                    unexpected.join(", ")
                )?;
            }
            if !duplicates.is_empty() {
                duplicates.sort_unstable();
                writeln!(
                    effects.get_error_stream(),
                    "{} found in the edited message multiple times:\n{}",
                    Pluralize {
                        determiner: Some(("This", "These")),
                        amount: duplicates.len(),
                        unit: ("commit was", "commits were"),
                    },
                    duplicates.join(", ")
                )?;
            }
            writeln!(
                effects.get_error_stream(),
                "Your edited message has been saved to .git/REWORD_EDITMSG for review and/or manual recovery."
            )?;
            return Ok(Err(ExitCode(1)));
        }
    };

    let rebase_plan = {
        let pool = ThreadPoolBuilder::new().build()?;
        let repo_pool = RepoResource::new_pool(&repo)?;
        let mut builder = RebasePlanBuilder::new(&dag, permissions);

        for commit in commits.iter() {
            let message = messages.get(&commit.get_oid()).unwrap();
            // This looks funny, but just means "leave everything but the message as is"
            let replacement_oid =
                commit.amend_commit(None, None, None, Some(message.as_str()), None)?;
            builder.move_subtree(commit.get_oid(), commit.get_parent_oids())?;
            builder.replace_commit(commit.get_oid(), replacement_oid)?;
        }

        match builder.build(effects, &pool, &repo_pool)? {
            Ok(Some(rebase_plan)) => rebase_plan,
            Ok(None) => {
                eyre::bail!(
                    "BUG: rebase plan indicates nothing to do, but rewording should always do something."
                );
            }
            Err(err) => {
                err.describe(effects, &repo, &dag)?;
                return Ok(Err(ExitCode(1)));
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
            reset: false,
            render_smartlog: false,
        },
        sign_option: sign_options.into(),
    };
    let result = execute_rebase_plan(
        effects,
        git_run_info,
        &repo,
        &event_log_db,
        &rebase_plan,
        &execute_options,
    )?;

    match result {
        ExecuteRebasePlanResult::Succeeded {
            rewritten_oids: Some(rewritten_oids),
        } => {
            render_status_report(&repo, effects, &commits, &rewritten_oids)?;
            Ok(Ok(()))
        }
        ExecuteRebasePlanResult::Succeeded {
            rewritten_oids: None,
        } => Ok(Ok(())),
        ExecuteRebasePlanResult::DeclinedToMerge {
            failed_merge_info: _,
        } => {
            writeln!(
                effects.get_error_stream(),
                "BUG: Merge failed, but rewording shouldn't cause any merge failures."
            )?;
            Ok(Err(ExitCode(1)))
        }
        ExecuteRebasePlanResult::Failed { exit_code } => Ok(Err(exit_code)),
    }
}

/// Turn a list of ref-ish strings into a list of Commits.
fn resolve_commits_from_hashes<'repo>(
    repo: &'repo Repo,
    dag: &mut Dag,
    effects: &Effects,
    revsets: Vec<Revset>,
    resolve_revset_options: &ResolveRevsetOptions,
) -> eyre::Result<Option<Vec<Commit<'repo>>>> {
    let commit_sets = match resolve_commits(effects, repo, dag, &revsets, resolve_revset_options) {
        Ok(commit_sets) => commit_sets,
        Err(err) => {
            err.describe(effects)?;
            return Ok(None);
        }
    };

    let commit_set = union_all(&commit_sets);
    let commits = sorted_commit_set(repo, dag, &commit_set)?;

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

    MismatchedCommits {
        duplicates: Vec<String>,
        missing: Vec<String>,
        unexpected: Vec<String>,
    },

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
        InitialCommitMessages::FixUp(_) => {
            eyre::bail!("BUG: Fixup should have already been handled!")
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

    let possible_template_message = message.trim();
    let possible_template_message = if possible_template_message.is_empty() {
        String::from("\n")
    } else {
        format!("{possible_template_message}\n\n")
    };
    let possible_template_message = possible_template_message.as_str();
    let discarded_message_header = format!("{comment_char} Original message:\n{comment_char} ");
    let discarded_message_header = discarded_message_header.as_str();
    let discarded_message_padding = format!("\n{comment_char} ");
    let discarded_message_padding = discarded_message_padding.as_str();

    let mut message = String::new();
    for commit in commits.iter() {
        let oid = commit.get_short_oid()?;

        let original_message = commit
            .get_message_raw()
            .to_str()
            .with_context(|| {
                eyre::eyre!(
                    "Could not decode commit message for commit: {:?}",
                    commit.get_oid()
                )
            })?
            .trim()
            .to_string();

        let msg = if discard_messages {
            [
                possible_template_message,
                discarded_message_header,
                original_message
                    .split('\n')
                    .collect::<Vec<&str>>()
                    .join(discarded_message_padding)
                    .as_str(),
            ]
            .concat()
        } else {
            original_message
        };

        let msg = if commits.len() == 1 {
            format!("{msg}\n\n")
        } else {
            format!("++ reword {oid}\n{msg}\n\n")
        };
        message.push_str(msg.as_str());
    }

    message.push_str(
        format!(
            "\
                {} Rewording: Please enter the commit {} to apply to {}.\n\
                {} Lines starting with '{}' will be ignored, and an empty message aborts\n\
                {} rewording.",
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

    let parsed_messages = parse_bulk_edit_message(message, commits, comment_char)?;

    let input_oids: HashSet<NonZeroOid> = commits.iter().map(|c| c.get_oid()).collect();
    let parsed_oids: HashSet<NonZeroOid> = parsed_messages.messages.keys().copied().collect();

    if input_oids != parsed_oids
        || !parsed_messages.duplicates.is_empty()
        || !parsed_messages.unexpected.is_empty()
    {
        let commits: HashMap<NonZeroOid, &Commit> = commits
            .iter()
            .map(|commit| (commit.get_oid(), commit))
            .collect();

        let mut missing = Vec::new();
        for oid in input_oids.difference(&parsed_oids) {
            let short_oid = match commits.get(oid) {
                Some(commit) => commit.get_short_oid()?,
                None => eyre::bail!(
                    "BUG: failed to retrieve known-good parsed OID from list of known-good input OIDs."
                ),
            };
            missing.push(short_oid);
        }

        let mut w = File::create(repo.get_path().join("REWORD_EDITMSG"))
            .context("Creating REWORD_EDITMSG file")?;
        use std::io::Write;
        writeln!(
            &mut w,
            "{} This file was created by `git branchless reword` at {}\n\
        {} You can use it to recover any edits you had made to the included commit {}.\n\
        {} If you don't need (or don't recognize) these edits, it is safe to delete this file.\n\
        \n\
        {}
        ",
            comment_char,
            Local::now().to_rfc2822(),
            comment_char,
            if commits.len() == 1 {
                "message"
            } else {
                "messages"
            },
            comment_char,
            edited_message
        )?;

        return Ok(PrepareMessagesResult::MismatchedCommits {
            duplicates: parsed_messages.duplicates,
            missing,
            unexpected: parsed_messages.unexpected,
        });
    }

    Ok(PrepareMessagesResult::Succeeded {
        messages: parsed_messages.messages,
    })
}

#[must_use]
#[derive(Debug)]
struct ParseMessageResult {
    /// Commit hashes that were found multiple times while parsing the edited messages.
    duplicates: Vec<String>,

    /// The parsed, formatted messages for rewording.
    messages: HashMap<NonZeroOid, String>,

    /// Commit hashes that were found while parsing the edited messages, but which were not
    /// specified on the command line.
    unexpected: Vec<String>,
}

#[instrument]
fn parse_bulk_edit_message(
    message: String,
    commits: &[Commit],
    comment_char: char,
) -> eyre::Result<ParseMessageResult> {
    let mut commits_oids = HashMap::new();
    for commit in commits.iter() {
        commits_oids.insert(commit.get_short_oid()?, commit.get_oid());
    }

    let message = match commits {
        // For single commits, add the marker line, but only if the user hasn't already done so.
        [only_commit] if !message.contains("++ reword") => {
            format!("++ reword {}\n{}", only_commit.get_short_oid()?, message)
        }
        _ => message,
    };

    // split the bulk message into (hash, msg) tuples
    let msgs = message
        .split("++ reword")
        .filter_map(|msg| msg.split_once('\n'))
        .map(|(hash, msg)| (hash.trim(), msg));

    let mut duplicates = Vec::new();
    let mut messages = HashMap::new();
    let mut unexpected = Vec::new();
    for (hash, msg) in msgs {
        let oid = match commits_oids.get(hash) {
            Some(commit) => *commit,
            None => {
                unexpected.push(hash.to_string());
                continue;
            }
        };
        if messages.contains_key(&oid) {
            duplicates.push(hash.to_string());
            continue;
        }
        messages.insert(oid, message_prettify(msg, Some(comment_char))?);
    }

    Ok(ParseMessageResult {
        duplicates,
        messages,
        unexpected,
    })
}

/// Return the root commits for given a list of commits. This is the list of commits that have *no*
/// ancestors also in the list. The idea is to find the minimum number of subtrees that much be
/// rebased to include all of our rewording.
#[instrument]
fn find_subtree_roots<'repo>(
    repo: &'repo Repo,
    dag: &Dag,
    commits: &[Commit],
) -> eyre::Result<Vec<Commit<'repo>>> {
    let commits: CommitSet = commits.iter().map(|commit| commit.get_oid()).collect();

    // Find the vertices representing the roots of this set of commits
    let subtree_roots = dag
        .query_roots(commits)
        .wrap_err("Computing subtree roots")?;

    // convert the vertices back into actual Commits
    let root_commits = dag
        .commit_set_to_vec(&subtree_roots)?
        .into_iter()
        .filter_map(|oid| repo.find_commit(oid).ok()?)
        .collect();

    Ok(root_commits)
}

/// Print a basic status report of what commits were reworded.
#[instrument]
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
            glyphs.render(
                // Commit doesn't offer `friendly_describe_oid`, so we'll do it ourselves
                render_node_descriptors(
                    &glyphs,
                    &NodeObject::Commit {
                        commit: original_commit.clone(),
                    },
                    &mut [&mut CommitOidDescriptor::new(true)?],
                )?
            )?,
            glyphs.render(replacement_commit.friendly_describe(&glyphs)?)?
        )?;
    }

    if num_commits != 1 {
        writeln!(
            effects.get_output_stream(),
            "Reworded {num_commits} commits. If this was unintentional, run: git undo",
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lib::testing::make_git;
    use std::collections::BTreeMap;

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
                    # Original message:
                    # create test1.txt

                    # Rewording: Please enter the commit message to apply to this 1 commit.
                    # Lines starting with '#' will be ignored, and an empty message aborts
                    # rewording.
                    "###);
                    Ok(message.to_string())
                },
            )?;
            insta::assert_debug_snapshot!(result, @"IdenticalMessage");
        }

        git.run(&["config", "commit.template", "template.txt"])?;
        git.write_file_txt(
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

                    # Original message:
                    # create test1.txt

                    # Rewording: Please enter the commit message to apply to this 1 commit.
                    # Lines starting with '#' will be ignored, and an empty message aborts
                    # rewording.
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

                    # Rewording: Please enter the commit messages to apply to these 2 commits.
                    # Lines starting with '#' will be ignored, and an empty message aborts
                    # rewording.
                    "###);
                    Ok(message.to_string())
                },
            )?;
            insta::assert_debug_snapshot!(result, @"IdenticalMessage");
        }

        Ok(())
    }

    #[test]
    fn test_reword_parses_bulk_edit_message() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        let repo = git.get_repo()?;

        let test1_oid = git.commit_file("test1", 1)?;
        let test2_oid = git.commit_file("test2", 2)?;
        let test1_commit = repo.find_commit_or_fail(test1_oid)?;
        let test2_commit = repo.find_commit_or_fail(test2_oid)?;

        {
            let mut result = parse_bulk_edit_message(
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

            // Convert the messages HashMap into the sorted map for testing
            let messages: BTreeMap<_, _> = result.messages.iter().collect();
            insta::assert_debug_snapshot!(messages, @r###"
                {
                    NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e): "create test1.txt\n",
                    NonZeroOid(96d1c37a3d4363611c49f7e52186e189a04c531f): "create test2.txt\n",
                }"###
            );

            // clear the messages map b/c its contents have already been tested
            result.messages.clear();
            insta::assert_debug_snapshot!(result, @r###"
                ParseMessageResult {
                    duplicates: [],
                    messages: {},
                    unexpected: [],
                }"###
            );
        };

        Ok(())
    }

    #[test]
    fn test_reword_parses_unexpected_and_duplicate_commit_hashs() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        let repo = git.get_repo()?;

        let test1_oid = git.commit_file("test1", 1)?;
        let test1_commit = repo.find_commit_or_fail(test1_oid)?;

        {
            let result = parse_bulk_edit_message(
                String::from(
                    "++ reword 62fc20d\n\
                create test1.txt\n\
                \n\
                ++ reword abc123\n\
                this commit doesn't exist\n\
                \n\
                ++ reword 62fc20d\n\
                this commit has been duplicated\n\
                \n",
                ),
                &[test1_commit.clone()],
                '#',
            )?;

            insta::assert_debug_snapshot!(result, @r###"
                ParseMessageResult {
                    duplicates: [
                        "62fc20d",
                    ],
                    messages: {
                        NonZeroOid(62fc20d2a290daea0d52bdc2ed2ad4be6491010e): "create test1.txt\n",
                    },
                    unexpected: [
                        "abc123",
                    ],
                }"###
            );
        };

        Ok(())
    }
}
