use std::fmt::Write;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::SystemTime;

use cursive::theme::{BaseColor, Effect, Style};
use cursive::utils::markup::StyledString;
use lazy_static::lazy_static;
use lib::core::dag::{sorted_commit_set, Dag};
use lib::core::effects::{icons, Effects, OperationIcon, OperationType};
use lib::core::eventlog::{EventLogDb, EventReplayer, EventTransactionId};
use lib::core::formatting::{Pluralize, StyledStringBuilder};
use lib::core::repo_ext::RepoExt;
use lib::core::rewrite::{
    execute_rebase_plan, ExecuteRebasePlanOptions, ExecuteRebasePlanResult, RebaseCommand,
    RebasePlan,
};
use lib::git::{Commit, GitRunInfo, GitRunResult, Repo};
use lib::util::{get_sh, ExitCode};

use crate::opts::Revset;
use crate::revset::resolve_commits;

lazy_static! {
    static ref STYLE_SUCCESS: Style =
        Style::merge(&[BaseColor::Green.light().into(), Effect::Bold.into()]);
    static ref STYLE_FAILURE: Style =
        Style::merge(&[BaseColor::Red.light().into(), Effect::Bold.into()]);
    static ref STYLE_SKIPPED: Style =
        Style::merge(&[BaseColor::Yellow.light().into(), Effect::Bold.into()]);
}

pub struct TestOptions {
    pub command: String,
}

pub fn run(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    options: &TestOptions,
    revset: Revset,
) -> eyre::Result<ExitCode> {
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "test")?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let references_snapshot = repo.get_references_snapshot()?;
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let commit_set = match resolve_commits(effects, &repo, &mut dag, &[revset]) {
        Ok(mut commit_sets) => commit_sets.pop().unwrap(),
        Err(err) => {
            err.describe(effects)?;
            return Ok(ExitCode(1));
        }
    };

    let abort_trap = match set_abort_trap(
        now,
        effects,
        git_run_info,
        &repo,
        &event_log_db,
        event_tx_id,
    )? {
        Ok(abort_trap) => abort_trap,
        Err(exit_code) => return Ok(exit_code),
    };

    let commits = sorted_commit_set(&repo, &dag, &commit_set)?;
    let result: Result<_, _> =
        run_tests(effects, git_run_info, &repo, event_tx_id, &commits, options);
    let abort_trap_exit_code = clear_abort_trap(effects, git_run_info, event_tx_id, abort_trap)?;
    if !abort_trap_exit_code.is_success() {
        return Ok(abort_trap_exit_code);
    }

    let result = result?;
    Ok(result)
}

#[must_use]
struct AbortTrap;

/// Ensure that no commit operation is currently underway (such as a merge or
/// rebase), and start a rebase.  In the event that the test invocation is
/// interrupted, this will prevent the user from starting another commit
/// operation without first running `git rebase --abort` to get back to their
/// original commit.
fn set_abort_trap(
    now: SystemTime,
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_log_db: &EventLogDb,
    event_tx_id: EventTransactionId,
) -> eyre::Result<Result<AbortTrap, ExitCode>> {
    if let Some(operation_type) = repo.get_current_operation_type() {
        writeln!(
            effects.get_output_stream(),
            "A {} operation is already in progress.",
            operation_type
        )?;
        writeln!(
            effects.get_output_stream(),
            "Run git {0} --continue or git {0} --abort to resolve it and proceed.",
            operation_type
        )?;
        return Ok(Err(ExitCode(1)));
    }

    let head_info = repo.get_head_info()?;
    let head_oid = match head_info.oid {
        Some(head_oid) => head_oid,
        None => {
            writeln!(
                effects.get_output_stream(),
                "No commit is currently checked out; cannot start on-disk rebase."
            )?;
            writeln!(
                effects.get_output_stream(),
                "Check out a commit and try again."
            )?;
            return Ok(Err(ExitCode(1)));
        }
    };

    let rebase_plan = RebasePlan {
        first_dest_oid: head_oid,
        commands: vec![RebaseCommand::Break],
    };
    match execute_rebase_plan(
        effects,
        git_run_info,
        repo,
        event_log_db,
        &rebase_plan,
        &ExecuteRebasePlanOptions {
            now,
            event_tx_id,
            preserve_timestamps: true,
            force_in_memory: false,
            force_on_disk: true,
            resolve_merge_conflicts: false,
            check_out_commit_options: Default::default(),
        },
    )? {
        ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ } => {
            // Do nothing.
        }
        ExecuteRebasePlanResult::DeclinedToMerge { merge_conflict } => {
            writeln!(
                effects.get_output_stream(),
                "BUG: Encountered unexpected merge conflict: {merge_conflict:?}"
            )?;
            return Ok(Err(ExitCode(1)));
        }
        ExecuteRebasePlanResult::Failed { exit_code } => {
            return Ok(Err(exit_code));
        }
    }

    Ok(Ok(AbortTrap))
}

fn clear_abort_trap(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    event_tx_id: EventTransactionId,
    _abort_trap: AbortTrap,
) -> eyre::Result<ExitCode> {
    let exit_code = git_run_info.run(effects, Some(event_tx_id), &["rebase", "--abort"])?;
    if !exit_code.is_success() {
        writeln!(
            effects.get_output_stream(),
            "{}",
            effects.get_glyphs().render(
                StyledStringBuilder::new()
                    .append_styled(
                        "Error: Could not abort tests with `git rebase --abort`.",
                        BaseColor::Red.light()
                    )
                    .build()
            )?
        )?;
    }
    Ok(exit_code)
}

enum TestOutput {
    CheckoutFailed,
    SpawnTestFailed(io::Error),
    TerminatedBySignal,
    Failed(i32),
    Passed,
}

impl TestOutput {
    fn describe(&self, effects: &Effects, commit: &Commit) -> eyre::Result<StyledString> {
        let glyphs = effects.get_glyphs();
        let description = match self {
            TestOutput::CheckoutFailed => StyledStringBuilder::new()
                .append_styled(
                    format!("{} Failed to check out: ", icons::EXCLAMATION),
                    *STYLE_SKIPPED,
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),
            TestOutput::SpawnTestFailed(err) => StyledStringBuilder::new()
                .append_styled(
                    format!("{} Failed to spawn test: {err}: ", icons::EXCLAMATION),
                    *STYLE_SKIPPED,
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),
            TestOutput::TerminatedBySignal => StyledStringBuilder::new()
                .append_styled(
                    format!("{} Test command terminated by signal: ", icons::CROSS),
                    *STYLE_FAILURE,
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),
            TestOutput::Failed(exit_code) => StyledStringBuilder::new()
                .append_styled(
                    format!("{} Failed with exit code {exit_code}: ", icons::CROSS),
                    *STYLE_FAILURE,
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),
            TestOutput::Passed => StyledStringBuilder::new()
                .append_styled(format!("{} Passed: ", icons::CHECKMARK), *STYLE_SUCCESS)
                .append(commit.friendly_describe(glyphs)?)
                .build(),
        };
        Ok(description)
    }
}

fn run_tests(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_tx_id: EventTransactionId,
    commits: &[Commit],
    options: &TestOptions,
) -> eyre::Result<ExitCode> {
    let TestOptions { command } = options;
    let shell_path = match get_sh() {
        Some(shell_path) => shell_path,
        None => {
            writeln!(
                effects.get_output_stream(),
                "{}",
                effects.get_glyphs().render(
                    StyledStringBuilder::new()
                        .append_styled(
                            "Error: Could not determine path to shell.",
                            BaseColor::Red.light()
                        )
                        .build()
                )?
            )?;
            return Ok(ExitCode(1));
        }
    };

    let results = {
        let (effects, progress) =
            effects.start_operation(OperationType::RunTests(Arc::new(command.clone())));
        progress.notify_progress(0, commits.len());
        let mut results = Vec::new();
        for commit in commits {
            {
                let (effects, progress) =
                    effects.start_operation(OperationType::RunTestOnCommit(Arc::new(
                        effects
                            .get_glyphs()
                            .render(commit.friendly_describe(effects.get_glyphs())?)?,
                    )));
                let output =
                    match prepare_working_directory(git_run_info, repo, event_tx_id, commit)? {
                        None => TestOutput::CheckoutFailed,
                        Some(working_directory) => {
                            test_commit(&working_directory, &shell_path, command, commit)?
                        }
                    };
                let text = output.describe(&effects, commit)?;
                progress.notify_status(
                    match output {
                        TestOutput::CheckoutFailed | TestOutput::SpawnTestFailed(_) => {
                            OperationIcon::Warning
                        }
                        TestOutput::TerminatedBySignal | TestOutput::Failed(_) => {
                            OperationIcon::Failure
                        }
                        TestOutput::Passed => OperationIcon::Success,
                    },
                    effects.get_glyphs().render(text)?,
                );
                results.push((commit, output));
            }
            progress.notify_progress_inc(1);
        }
        results
    };

    writeln!(
        effects.get_output_stream(),
        "Ran {} on {}:",
        effects.get_glyphs().render(
            StyledStringBuilder::new()
                .append_styled(command, Effect::Bold)
                .build()
        )?,
        Pluralize {
            determiner: None,
            amount: commits.len(),
            unit: ("commit", "commits")
        }
    )?;
    let mut num_passed = 0;
    let mut num_failed = 0;
    let mut num_skipped = 0;
    for (commit, output) in results {
        writeln!(
            effects.get_output_stream(),
            "{}",
            effects
                .get_glyphs()
                .render(output.describe(effects, commit)?)?
        )?;
        match output {
            TestOutput::CheckoutFailed
            | TestOutput::SpawnTestFailed(_)
            | TestOutput::TerminatedBySignal => num_skipped += 1,
            TestOutput::Failed(_) => num_failed += 1,
            TestOutput::Passed => num_passed += 1,
        }
    }

    let passed = effects.get_glyphs().render(
        StyledStringBuilder::new()
            .append_styled(format!("{num_passed} passed"), *STYLE_SUCCESS)
            .build(),
    )?;
    let failed = effects.get_glyphs().render(
        StyledStringBuilder::new()
            .append_styled(format!("{num_failed} failed"), *STYLE_FAILURE)
            .build(),
    )?;
    let skipped = effects.get_glyphs().render(
        StyledStringBuilder::new()
            .append_styled(format!("{num_skipped} skipped"), *STYLE_SKIPPED)
            .build(),
    )?;
    writeln!(effects.get_output_stream(), "{passed}, {failed}, {skipped}")?;

    if num_failed > 0 || num_skipped > 0 {
        Ok(ExitCode(1))
    } else {
        Ok(ExitCode(0))
    }
}

fn prepare_working_directory(
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_tx_id: EventTransactionId,
    commit: &Commit,
) -> eyre::Result<Option<PathBuf>> {
    let GitRunResult { exit_code, stdout: _, stderr: _ } =
        // Don't show the `git checkout` operation among the progress bars, as we only want to see
        // the testing status.
        git_run_info.run_silent(
            repo,
            Some(event_tx_id),
            &["checkout", &commit.get_oid().to_string()],
            Default::default()
        )?;
    if exit_code.is_success() {
        Ok(repo.get_working_copy_path().map(|path| path.to_owned()))
    } else {
        Ok(None)
    }
}

fn test_commit(
    working_directory: &Path,
    shell_path: &Path,
    command: &str,
    _commit: &Commit,
) -> eyre::Result<TestOutput> {
    let exit_code = match Command::new(&shell_path)
        .arg("-c")
        .arg(command)
        .current_dir(working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) => output.status.code(),
        Err(err) => {
            return Ok(TestOutput::SpawnTestFailed(err));
        }
    };

    let exit_code = match exit_code {
        Some(exit_code) => exit_code,
        None => {
            return Ok(TestOutput::TerminatedBySignal);
        }
    };

    let output = match exit_code {
        0 => TestOutput::Passed,
        exit_code => TestOutput::Failed(exit_code),
    };
    Ok(output)
}
