//! Commit changes in the working copy.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_conditions)]

use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt::Write;
use std::time::SystemTime;

use git_branchless_invoke::CommandContext;
use git_branchless_opts::RecordArgs;
use git_branchless_reword::edit_message;
use itertools::Itertools;
use lib::core::check_out::{check_out_commit, CheckOutCommitOptions, CheckoutTarget};
use lib::core::config::{get_commit_template, get_restack_preserve_timestamps};
use lib::core::dag::{CommitSet, Dag};
use lib::core::effects::{Effects, OperationType};
use lib::core::eventlog::{EventLogDb, EventReplayer, EventTransactionId};
use lib::core::formatting::Pluralize;
use lib::core::repo_ext::RepoExt;
use lib::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanError, BuildRebasePlanOptions, ExecuteRebasePlanOptions,
    ExecuteRebasePlanResult, MergeConflictRemediation, RebasePlanBuilder, RebasePlanPermissions,
    RepoResource,
};
use lib::git::{
    process_diff_for_record, update_index, CategorizedReferenceName, FileMode, GitRunInfo,
    MaybeZeroOid, NonZeroOid, Repo, ResolvedReferenceInfo, Stage, UpdateIndexCommand,
    WorkingCopyChangesType, WorkingCopySnapshot,
};
use lib::try_exit_code;
use lib::util::{ExitCode, EyreExitOr};
use rayon::ThreadPoolBuilder;
use scm_record::helpers::CrosstermInput;
use scm_record::{
    Commit, Event, RecordError, RecordInput, RecordState, Recorder, SelectedContents, TerminalKind,
};
use tracing::{instrument, warn};

/// Commit changes in the working copy.
#[instrument]
pub fn command_main(ctx: CommandContext, args: RecordArgs) -> EyreExitOr<()> {
    let CommandContext {
        effects,
        git_run_info,
    } = ctx;
    let RecordArgs {
        messages,
        interactive,
        create,
        detach,
        insert,
        stash,
    } = args;
    record(
        &effects,
        &git_run_info,
        messages,
        interactive,
        create,
        detach,
        insert,
        stash,
    )
}

#[instrument]
fn record(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    messages: Vec<String>,
    interactive: bool,
    branch_name: Option<String>,
    detach: bool,
    insert: bool,
    stash: bool,
) -> EyreExitOr<()> {
    let now = SystemTime::now();
    let repo = Repo::from_dir(&git_run_info.working_directory)?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "record")?;

    let (snapshot, working_copy_changes_type) = {
        let head_info = repo.get_head_info()?;
        let index = repo.get_index()?;
        let (snapshot, _status) =
            repo.get_status(effects, git_run_info, &index, &head_info, Some(event_tx_id))?;

        let working_copy_changes_type = snapshot.get_working_copy_changes_type()?;
        match working_copy_changes_type {
            WorkingCopyChangesType::None => {
                writeln!(
                    effects.get_output_stream(),
                    "There are no changes to tracked files in the working copy to commit."
                )?;
                return Ok(Ok(()));
            }
            WorkingCopyChangesType::Unstaged | WorkingCopyChangesType::Staged => {}
            WorkingCopyChangesType::Conflicts => {
                writeln!(
                    effects.get_output_stream(),
                    "Cannot commit changes while there are unresolved merge conflicts."
                )?;
                writeln!(
                    effects.get_output_stream(),
                    "Resolve them and try again. Aborting."
                )?;
                return Ok(Err(ExitCode(1)));
            }
        }
        (snapshot, working_copy_changes_type)
    };

    if let Some(branch_name) = branch_name {
        try_exit_code!(check_out_commit(
            effects,
            git_run_info,
            &repo,
            &event_log_db,
            event_tx_id,
            None,
            &CheckOutCommitOptions {
                additional_args: vec![OsString::from("-b"), OsString::from(branch_name)],
                reset: false,
                render_smartlog: false,
            },
        )?);
    }

    if interactive {
        if working_copy_changes_type == WorkingCopyChangesType::Staged {
            writeln!(
                effects.get_output_stream(),
                "Cannot select changes interactively while there are already staged changes."
            )?;
            writeln!(
                effects.get_output_stream(),
                "Either commit or unstage your changes and try again. Aborting."
            )?;
            return Ok(Err(ExitCode(1)));
        } else {
            try_exit_code!(record_interactive(
                effects,
                git_run_info,
                &repo,
                &snapshot,
                event_tx_id,
                messages,
            )?);
        }
    } else {
        let args = {
            let mut args = vec!["commit"];
            args.extend(messages.iter().flat_map(|message| ["--message", message]));
            if working_copy_changes_type == WorkingCopyChangesType::Unstaged {
                args.push("--all");
            }
            args
        };
        try_exit_code!(git_run_info.run_direct_no_wrapping(Some(event_tx_id), &args)?);
    }

    if detach || stash {
        let head_info = repo.get_head_info()?;
        if let ResolvedReferenceInfo {
            oid: Some(oid),
            reference_name: Some(reference_name),
        } = &head_info
        {
            let head_commit = repo.find_commit_or_fail(*oid)?;
            match head_commit.get_parents().as_slice() {
                [] => try_exit_code!(git_run_info.run(
                    effects,
                    Some(event_tx_id),
                    &[
                        "update-ref",
                        "-d",
                        reference_name.as_str(),
                        &oid.to_string(),
                    ],
                )?),
                [parent_commit] => {
                    let branch_name = CategorizedReferenceName::new(reference_name).render_suffix();
                    repo.detach_head(&head_info)?;
                    try_exit_code!(git_run_info.run(
                        effects,
                        Some(event_tx_id),
                        &[
                            "branch",
                            "-f",
                            &branch_name,
                            &parent_commit.get_oid().to_string(),
                        ],
                    )?);
                }
                parent_commits => {
                    eyre::bail!("git-branchless record --detach called on a merge commit, but it should only be capable of creating zero- or one-parent commits. Parents: {parent_commits:?}");
                }
            }
        }
        let checkout_target = match head_info {
            ResolvedReferenceInfo {
                oid: _,
                reference_name: Some(reference_name),
            } => Some(CheckoutTarget::Reference(reference_name.clone())),
            ResolvedReferenceInfo {
                oid: Some(oid),
                reference_name: _,
            } => Some(CheckoutTarget::Oid(oid)),
            _ => None,
        };
        if stash && checkout_target.is_some() {
            try_exit_code!(check_out_commit(
                effects,
                git_run_info,
                &repo,
                &event_log_db,
                event_tx_id,
                checkout_target,
                &CheckOutCommitOptions {
                    additional_args: vec![],
                    reset: false,
                    render_smartlog: false,
                },
            )?);
        }
    }

    if insert {
        try_exit_code!(insert_before_siblings(
            effects,
            git_run_info,
            now,
            event_tx_id
        )?);
    }

    Ok(Ok(()))
}

#[instrument]
fn record_interactive(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    snapshot: &WorkingCopySnapshot,
    event_tx_id: EventTransactionId,
    messages: Vec<String>,
) -> EyreExitOr<()> {
    let old_tree = snapshot.commit_stage0.get_tree()?;
    let new_tree = snapshot.commit_unstaged.get_tree()?;
    let files = {
        let (effects, _progress) = effects.start_operation(OperationType::CalculateDiff);
        let diff = repo.get_diff_between_trees(
            &effects,
            Some(&old_tree),
            &new_tree,
            // We manually add context to the git-record output, so suppress the context lines here.
            0,
        )?;
        process_diff_for_record(repo, &diff)?
    };
    let record_state = RecordState {
        is_read_only: false,
        commits: vec![
            Commit {
                message: Some(messages.iter().join("\n\n")),
            },
            Commit { message: None },
        ],
        files,
    };

    struct Input<'a> {
        git_run_info: &'a GitRunInfo,
        repo: &'a Repo,
    }
    impl RecordInput for Input<'_> {
        fn terminal_kind(&self) -> TerminalKind {
            TerminalKind::Crossterm
        }

        fn next_events(&mut self) -> Result<Vec<Event>, RecordError> {
            CrosstermInput.next_events()
        }

        fn edit_commit_message(&mut self, message: &str) -> Result<String, RecordError> {
            let Self { git_run_info, repo } = self;
            let commit_template = get_commit_template(repo).map_err(|err| {
                RecordError::Other(format!("Could not read commit message template: {err}",))
            })?;
            let message = if message.is_empty() {
                commit_template.as_deref().unwrap_or("")
            } else {
                message
            };
            edit_message(git_run_info, repo, message)
                .map_err(|err| RecordError::Other(err.to_string()))
        }
    }
    let mut input = Input { git_run_info, repo };
    let recorder = Recorder::new(record_state, &mut input);
    let result = recorder.run();
    let RecordState {
        is_read_only: _,
        commits,
        files: result,
    } = match result {
        Ok(result) => result,
        Err(RecordError::Cancelled) => {
            println!("Aborted.");
            return Ok(Err(ExitCode(1)));
        }
        Err(RecordError::Bug(message)) => {
            println!("BUG: {message}");
            println!("This is a bug. Please report it.");
            return Ok(Err(ExitCode(1)));
        }
        Err(
            err @ (RecordError::SetUpTerminal(_)
            | RecordError::CleanUpTerminal(_)
            | RecordError::ReadInput(_)
            | RecordError::RenderFrame(_)
            | RecordError::SerializeJson(_)
            | RecordError::WriteFile(_)
            | RecordError::Other(_)),
        ) => {
            println!("Error: {err}");
            return Ok(Err(ExitCode(1)));
        }
    };
    let message = commits[0].message.clone().unwrap_or_default();

    let update_index_script: Vec<UpdateIndexCommand> = result
        .into_iter()
        .map(|file| -> eyre::Result<UpdateIndexCommand> {
            let mode = {
                let default_mode = FileMode::Blob;
                match file.get_file_mode() {
                    None => {
                        warn!(
                            ?file,
                            ?default_mode,
                            "No file mode was set for file, using default"
                        );
                        default_mode
                    }
                    Some(mode) => match i32::try_from(mode) {
                        Ok(mode) => FileMode::from(mode),
                        Err(err) => {
                            warn!(
                                ?mode,
                                ?default_mode,
                                ?err,
                                "File mode did not fit into i32, using default"
                            );
                            default_mode
                        }
                    },
                }
            };

            let (selected, _unselected) = file.get_selected_contents();
            let oid = match selected {
                SelectedContents::Absent => MaybeZeroOid::Zero,
                SelectedContents::Unchanged => {
                    old_tree.get_oid_for_path(&file.path)?.unwrap_or_default()
                }
                SelectedContents::Binary {
                    old_description: _,
                    new_description: _,
                } => new_tree.get_oid_for_path(&file.path)?.unwrap(),
                SelectedContents::Present { contents } => {
                    MaybeZeroOid::NonZero(repo.create_blob_from_contents(contents.as_bytes())?)
                }
            };
            let command = match oid {
                MaybeZeroOid::Zero => UpdateIndexCommand::Delete {
                    path: file.path.clone().into_owned(),
                },
                MaybeZeroOid::NonZero(oid) => UpdateIndexCommand::Update {
                    path: file.path.clone().into_owned(),
                    stage: Stage::Stage0,
                    mode,
                    oid,
                },
            };
            Ok(command)
        })
        .try_collect()?;
    let index = repo.get_index()?;
    update_index(
        git_run_info,
        repo,
        &index,
        event_tx_id,
        &update_index_script,
    )?;

    let args = {
        let mut args = vec!["commit"];
        if !message.is_empty() {
            args.extend(["--message", &message]);
        }
        args
    };
    git_run_info.run_direct_no_wrapping(Some(event_tx_id), &args)
}

#[instrument]
fn insert_before_siblings(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    now: SystemTime,
    event_tx_id: EventTransactionId,
) -> EyreExitOr<()> {
    // Reopen the repository since references may have changed.
    let repo = Repo::from_dir(&git_run_info.working_directory)?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let references_snapshot = repo.get_references_snapshot()?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let head_info = repo.get_head_info()?;
    let head_oid = match head_info {
        ResolvedReferenceInfo {
            oid: Some(head_oid),
            reference_name: _,
        } => head_oid,
        ResolvedReferenceInfo {
            oid: None,
            reference_name: _,
        } => {
            return Ok(Ok(()));
        }
    };

    let dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;
    let head_commit = repo.find_commit_or_fail(head_oid)?;
    let head_commit_set = CommitSet::from(head_oid);
    let parents = dag.query_parents(head_commit_set.clone())?;
    let children = dag.query_children(parents)?;
    let siblings = children.difference(&head_commit_set);
    let siblings = dag.filter_visible_commits(siblings)?;
    let build_options = BuildRebasePlanOptions {
        force_rewrite_public_commits: false,
        dump_rebase_constraints: false,
        dump_rebase_plan: false,
        detect_duplicate_commits_via_patch_id: true,
    };

    let rebase_plan_result =
        match RebasePlanPermissions::verify_rewrite_set(&dag, build_options, &siblings)? {
            Err(err) => Err(err),
            Ok(permissions) => {
                let head_commit_parents: HashSet<_> =
                    head_commit.get_parent_oids().into_iter().collect();
                let mut builder = RebasePlanBuilder::new(&dag, permissions);
                for sibling_oid in dag.commit_set_to_vec(&siblings)? {
                    let sibling_commit = repo.find_commit_or_fail(sibling_oid)?;
                    let parent_oids = sibling_commit.get_parent_oids();
                    let new_parent_oids = parent_oids
                        .into_iter()
                        .map(|parent_oid| {
                            if head_commit_parents.contains(&parent_oid) {
                                head_oid
                            } else {
                                parent_oid
                            }
                        })
                        .collect_vec();
                    builder.move_subtree(sibling_oid, new_parent_oids)?;
                }
                let thread_pool = ThreadPoolBuilder::new().build()?;
                let repo_pool = RepoResource::new_pool(&repo)?;
                builder.build(effects, &thread_pool, &repo_pool)?
            }
        };

    let rebase_plan = match rebase_plan_result {
        Ok(Some(rebase_plan)) => rebase_plan,

        Ok(None) => {
            // Nothing to do, since there were no siblings to move.
            return Ok(Ok(()));
        }

        Err(BuildRebasePlanError::ConstraintCycle { .. }) => {
            writeln!(
                effects.get_output_stream(),
                "BUG: constraint cycle detected when moving siblings, which shouldn't be possible."
            )?;
            return Ok(Err(ExitCode(1)));
        }

        Err(err @ BuildRebasePlanError::MoveIllegalCommits { .. }) => {
            err.describe(effects, &repo, &dag)?;
            return Ok(Err(ExitCode(1)));
        }

        Err(BuildRebasePlanError::MovePublicCommits {
            public_commits_to_move,
        }) => {
            let example_bad_commit_oid = dag
                .set_first(&public_commits_to_move)?
                .ok_or_else(|| eyre::eyre!("BUG: could not get OID of a public commit to move"))?;
            let example_bad_commit_oid = NonZeroOid::try_from(example_bad_commit_oid)?;
            let example_bad_commit = repo.find_commit_or_fail(example_bad_commit_oid)?;
            writeln!(
                effects.get_output_stream(),
                "\
You are trying to rewrite {}, such as: {}
It is generally not advised to rewrite public commits, because your
collaborators will have difficulty merging your changes.
To proceed anyways, run: git move -f -s 'siblings(.)",
                Pluralize {
                    determiner: None,
                    amount: dag.set_count(&public_commits_to_move)?,
                    unit: ("public commit", "public commits")
                },
                effects
                    .get_glyphs()
                    .render(example_bad_commit.friendly_describe(effects.get_glyphs())?)?,
            )?;
            return Ok(Ok(()));
        }
    };

    let execute_options = ExecuteRebasePlanOptions {
        now,
        event_tx_id,
        preserve_timestamps: get_restack_preserve_timestamps(&repo)?,
        force_in_memory: true,
        force_on_disk: false,
        resolve_merge_conflicts: false,
        check_out_commit_options: Default::default(),
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
        ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ } => Ok(Ok(())),
        ExecuteRebasePlanResult::DeclinedToMerge { failed_merge_info } => {
            failed_merge_info.describe(effects, &repo, MergeConflictRemediation::Insert)?;
            Ok(Ok(()))
        }
        ExecuteRebasePlanResult::Failed { exit_code } => Ok(Err(exit_code)),
    }
}
