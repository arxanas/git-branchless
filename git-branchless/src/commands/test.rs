use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::SystemTime;

use clap::ValueEnum;
use cursive::theme::{BaseColor, Effect, Style};
use cursive::utils::markup::StyledString;
use eyre::WrapErr;
use fslock::LockFile;
use itertools::Itertools;
use lazy_static::lazy_static;
use lib::core::config::{get_hint_enabled, get_hint_string, print_hint_suppression_notice, Hint};
use lib::core::dag::{sorted_commit_set, Dag};
use lib::core::effects::{icons, Effects, OperationIcon, OperationType};
use lib::core::eventlog::{EventLogDb, EventReplayer, EventTransactionId};
use lib::core::formatting::{Pluralize, StyledStringBuilder};
use lib::core::repo_ext::RepoExt;
use lib::core::rewrite::{
    execute_rebase_plan, ExecuteRebasePlanOptions, ExecuteRebasePlanResult, RebaseCommand,
    RebasePlan,
};
use lib::git::{Commit, ConfigRead, GitRunInfo, GitRunResult, Repo};
use lib::util::{get_sh, ExitCode};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::opts::{Revset, TestExecutionStrategy};
use crate::revset::resolve_commits;

lazy_static! {
    static ref STYLE_SUCCESS: Style =
        Style::merge(&[BaseColor::Green.light().into(), Effect::Bold.into()]);
    static ref STYLE_FAILURE: Style =
        Style::merge(&[BaseColor::Red.light().into(), Effect::Bold.into()]);
    static ref STYLE_SKIPPED: Style =
        Style::merge(&[BaseColor::Yellow.light().into(), Effect::Bold.into()]);
}

#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum Verbosity {
    None,
    PartialOutput,
    FullOutput,
}

impl From<u8> for Verbosity {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::PartialOutput,
            _ => Self::FullOutput,
        }
    }
}

#[derive(Debug)]
pub struct RawTestOptions {
    pub exec: Option<String>,
    pub command: Option<String>,
    pub strategy: Option<TestExecutionStrategy>,
    pub verbosity: Verbosity,
}

fn resolve_test_command_alias(
    effects: &Effects,
    repo: &Repo,
    alias: Option<&str>,
) -> eyre::Result<Result<String, ExitCode>> {
    let config = repo.get_readonly_config()?;
    let config_key = format!("branchless.test.alias.{}", alias.unwrap_or("default"));
    let config_value: Option<String> = config.get(config_key).unwrap_or_default();
    if let Some(command) = config_value {
        return Ok(Ok(command));
    }

    match alias {
        Some(alias) => {
            writeln!(
                effects.get_output_stream(),
                "\
The test command alias {alias:?} was not defined.

To create it, run: git config branchless.test.alias.{alias} <command>
Or use the -x/--exec flag instead to run a test command without first creating an alias."
            )?;
        }
        None => {
            writeln!(
                effects.get_output_stream(),
                "\
Could not determine test command to run. No test command was provided with -c/--command or
-x/--exec, and the configuration value 'branchless.test.alias.default' was not set.

To configure a default test command, run: git config branchless.test.alias.default <command>
To run a specific test command, run: git test run -x <command>
To run a specific command alias, run: git test run -c <alias>",
            )?;
        }
    }

    let aliases = config.list("branchless.test.alias.*")?;
    if !aliases.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "\nThese are the currently-configured command aliases:"
        )?;
        for (name, command) in aliases {
            writeln!(
                effects.get_output_stream(),
                "{} {name} = {command:?}",
                effects.get_glyphs().bullet_point,
            )?;
        }
    }

    Ok(Err(ExitCode(1)))
}

#[derive(Debug)]
struct ResolvedTestOptions {
    command: String,
    strategy: TestExecutionStrategy,
    verbosity: Verbosity,
}

impl ResolvedTestOptions {
    fn resolve(
        effects: &Effects,
        repo: &Repo,
        options: &RawTestOptions,
    ) -> eyre::Result<Result<Self, ExitCode>> {
        let config = repo.get_readonly_config()?;
        let RawTestOptions {
            exec: command,
            command: command_alias,
            strategy,
            verbosity,
        } = options;
        let resolved_command = match (command, command_alias) {
            (Some(command), None) => command.to_owned(),
            (None, maybe_command_alias) => {
                match resolve_test_command_alias(effects, repo, maybe_command_alias.as_deref())? {
                    Ok(command) => command,
                    Err(exit_code) => {
                        return Ok(Err(exit_code));
                    }
                }
            }
            (Some(command), Some(command_alias)) => unreachable!(
                "Command ({:?}) and command alias ({:?}) are conflicting options",
                command, command_alias
            ),
        };
        let resolved_strategy = match strategy {
            Some(strategy) => *strategy,
            None => {
                let strategy: Option<String> = config.get("branchless.test.strategy")?;
                match strategy {
                    None => TestExecutionStrategy::WorkingCopy,
                    Some(strategy) => match TestExecutionStrategy::from_str(&strategy, true) {
                        Ok(strategy) => strategy,
                        Err(_) => {
                            writeln!(effects.get_output_stream(), "Invalid value for config value branchless.test.strategy: {strategy}")?;
                            writeln!(
                                effects.get_output_stream(),
                                "Expected one of: {}",
                                TestExecutionStrategy::value_variants()
                                    .iter()
                                    .filter_map(|variant| variant.to_possible_value())
                                    .map(|value| value.get_name().to_owned())
                                    .join(", ")
                            )?;
                            return Ok(Err(ExitCode(1)));
                        }
                    },
                }
            }
        };
        Ok(Ok(ResolvedTestOptions {
            command: resolved_command,
            strategy: resolved_strategy,
            verbosity: *verbosity,
        }))
    }

    fn make_command_slug(&self) -> String {
        self.command.replace(&['/', ' ', '\n'], "_")
    }
}

#[instrument]
pub fn run(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    options: &RawTestOptions,
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

    let options = match ResolvedTestOptions::resolve(effects, &repo, options)? {
        Ok(options) => options,
        Err(exit_code) => return Ok(exit_code),
    };

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
        options.strategy,
    )? {
        Ok(abort_trap) => abort_trap,
        Err(exit_code) => return Ok(exit_code),
    };

    let commits = sorted_commit_set(&repo, &dag, &commit_set)?;
    let result: Result<_, _> = run_tests(
        effects,
        git_run_info,
        &repo,
        event_tx_id,
        &commits,
        &options,
    );
    let abort_trap_exit_code = clear_abort_trap(effects, git_run_info, event_tx_id, abort_trap)?;
    if !abort_trap_exit_code.is_success() {
        return Ok(abort_trap_exit_code);
    }

    let result = result?;
    Ok(result)
}

#[must_use]
#[derive(Debug)]
struct AbortTrap {
    is_active: bool,
}

/// Ensure that no commit operation is currently underway (such as a merge or
/// rebase), and start a rebase.  In the event that the test invocation is
/// interrupted, this will prevent the user from starting another commit
/// operation without first running `git rebase --abort` to get back to their
/// original commit.
#[instrument]
fn set_abort_trap(
    now: SystemTime,
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_log_db: &EventLogDb,
    event_tx_id: EventTransactionId,
    strategy: TestExecutionStrategy,
) -> eyre::Result<Result<AbortTrap, ExitCode>> {
    match strategy {
        TestExecutionStrategy::Worktree => return Ok(Ok(AbortTrap { is_active: false })),
        TestExecutionStrategy::WorkingCopy => {}
    }

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

    Ok(Ok(AbortTrap { is_active: true }))
}

#[instrument]
fn clear_abort_trap(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    event_tx_id: EventTransactionId,
    abort_trap: AbortTrap,
) -> eyre::Result<ExitCode> {
    let AbortTrap { is_active } = abort_trap;
    if !is_active {
        return Ok(ExitCode(0));
    }

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

#[derive(Debug)]
struct TestOutput {
    _result_path: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    test_status: TestStatus,
}

/// The possible results of attempting to run a test.
#[derive(Debug, Serialize, Deserialize)]
enum TestStatus {
    /// Attempting to set up the working directory for the repository failed.
    CheckoutFailed,

    /// Invoking the test command failed.
    SpawnTestFailed(String),

    /// The test command was invoked successfully, but was terminated by a signal, rather than
    /// returning an exit code normally.
    TerminatedBySignal,

    /// It appears that some other process is already running the test for a commit with the given
    /// tree. (If that process crashed, then the test may need to be re-run.)
    AlreadyInProgress,

    /// Attempting to read cached data failed.
    ReadCacheFailed(String),

    /// The test failed and returned the provided (non-zero) exit code.
    Failed(i32),

    /// Like [`Failed`], but the result was cached, so we didn't need to re-run the test.
    FailedCached(i32),

    /// The test passed and returned a successful exit code.
    Passed,

    /// Like [`Passed`], but the result was cached, so we didn't need to re-run the test.
    PassedCached,
}

impl TestOutput {
    #[instrument]
    fn describe(
        &self,
        effects: &Effects,
        commit: &Commit,
        verbosity: Verbosity,
    ) -> eyre::Result<StyledString> {
        let glyphs = effects.get_glyphs();
        let description = match &self.test_status {
            TestStatus::CheckoutFailed => StyledStringBuilder::new()
                .append_styled(
                    format!("{} Failed to check out: ", icons::EXCLAMATION),
                    *STYLE_SKIPPED,
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::SpawnTestFailed(err) => StyledStringBuilder::new()
                .append_styled(
                    format!("{} Failed to spawn test: {err}: ", icons::EXCLAMATION),
                    *STYLE_SKIPPED,
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::TerminatedBySignal => StyledStringBuilder::new()
                .append_styled(
                    format!("{} Test command terminated by signal: ", icons::CROSS),
                    *STYLE_FAILURE,
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::AlreadyInProgress => StyledStringBuilder::new()
                .append_styled(
                    format!("{} Test already in progress? ", icons::EXCLAMATION),
                    *STYLE_SKIPPED,
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::ReadCacheFailed(_) => StyledStringBuilder::new()
                .append_styled(
                    format!("{} Could not read cached test result: ", icons::EXCLAMATION),
                    *STYLE_SKIPPED,
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::Failed(exit_code) => StyledStringBuilder::new()
                .append_styled(
                    format!("{} Failed with exit code {exit_code}: ", icons::CROSS),
                    *STYLE_FAILURE,
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::FailedCached(exit_code) => StyledStringBuilder::new()
                .append_styled(
                    format!(
                        "{} Failed (cached) with exit code {exit_code}: ",
                        icons::CROSS
                    ),
                    *STYLE_FAILURE,
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::Passed => StyledStringBuilder::new()
                .append_styled(format!("{} Passed: ", icons::CHECKMARK), *STYLE_SUCCESS)
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::PassedCached => StyledStringBuilder::new()
                .append_styled(
                    format!("{} Passed (cached): ", icons::CHECKMARK),
                    *STYLE_SUCCESS,
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),
        };

        if verbosity == Verbosity::None {
            return Ok(StyledStringBuilder::new()
                .append(description)
                .append_plain("\n")
                .build());
        }

        fn abbreviate_lines(path: &Path, verbosity: Verbosity) -> Vec<StyledString> {
            let should_show_all_lines = match verbosity {
                Verbosity::None => return Vec::new(),
                Verbosity::PartialOutput => false,
                Verbosity::FullOutput => true,
            };

            // FIXME: don't read entire file into memory
            let contents = match std::fs::read_to_string(path) {
                Ok(contents) => contents,
                Err(_) => {
                    return vec![StyledStringBuilder::new()
                        .append_plain("<failed to read file>")
                        .build()]
                }
            };

            const NUM_CONTEXT_LINES: usize = 5;
            let lines = contents.lines().collect_vec();
            let num_missing_lines = lines.len().saturating_sub(2 * NUM_CONTEXT_LINES);
            let num_missing_lines_message = format!("<{num_missing_lines} more lines>");
            let lines = if lines.is_empty() {
                vec!["<no output>"]
            } else if num_missing_lines == 0 || should_show_all_lines {
                lines
            } else {
                [
                    &lines[..NUM_CONTEXT_LINES],
                    &[num_missing_lines_message.as_str()],
                    &lines[lines.len() - NUM_CONTEXT_LINES..],
                ]
                .concat()
            };
            lines
                .into_iter()
                .map(|line| StyledStringBuilder::new().append_plain(line).build())
                .collect()
        }

        let stdout_path_line = StyledStringBuilder::new()
            .append_styled("Stdout: ", Effect::Bold)
            .append_plain(self.stdout_path.to_string_lossy())
            .build();
        let stdout_lines = abbreviate_lines(&self.stdout_path, verbosity);
        let stderr_path_line = StyledStringBuilder::new()
            .append_styled("Stderr: ", Effect::Bold)
            .append_plain(self.stderr_path.to_string_lossy())
            .build();
        let stderr_lines = abbreviate_lines(&self.stderr_path, verbosity);

        Ok(StyledStringBuilder::from_lines(
            [
                &[description],
                &[stdout_path_line],
                stdout_lines.as_slice(),
                &[stderr_path_line],
                stderr_lines.as_slice(),
            ]
            .concat(),
        ))
    }
}

#[instrument]
fn run_tests(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_tx_id: EventTransactionId,
    commits: &[Commit],
    options: &ResolvedTestOptions,
) -> eyre::Result<ExitCode> {
    let ResolvedTestOptions {
        command,
        strategy: _,
        verbosity,
    } = &options;

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
            let test_output = run_test(
                &effects,
                git_run_info,
                &shell_path,
                repo,
                event_tx_id,
                options,
                commit,
            )?;
            results.push((commit, test_output));
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
    let mut num_cached_results = 0;
    for (commit, test_output) in results {
        write!(
            effects.get_output_stream(),
            "{}",
            effects
                .get_glyphs()
                .render(test_output.describe(effects, commit, *verbosity)?)?
        )?;
        match test_output.test_status {
            TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_)
            | TestStatus::TerminatedBySignal => num_skipped += 1,

            TestStatus::Failed(_) => {
                num_failed += 1;
            }
            TestStatus::FailedCached(_) => {
                num_failed += 1;
                num_cached_results += 1;
            }
            TestStatus::Passed => {
                num_passed += 1;
            }
            TestStatus::PassedCached => {
                num_passed += 1;
                num_cached_results += 1;
            }
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

    if num_cached_results > 0 && get_hint_enabled(repo, Hint::CleanCachedTestResults)? {
        writeln!(
            effects.get_output_stream(),
            "{}: There {}.",
            effects.get_glyphs().render(get_hint_string())?,
            Pluralize {
                determiner: Some(("was", "were")),
                amount: num_cached_results,
                unit: ("cached test result", "cached test results")
            }
        )?;
        writeln!(
            effects.get_output_stream(),
            "{}: To clear all cached results, run: git test clean",
            effects.get_glyphs().render(get_hint_string())?,
        )?;
        print_hint_suppression_notice(effects, Hint::CleanCachedTestResults)?;
    }

    if num_failed > 0 || num_skipped > 0 {
        Ok(ExitCode(1))
    } else {
        Ok(ExitCode(0))
    }
}

#[instrument]
fn run_test(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    shell_path: &Path,
    repo: &Repo,
    event_tx_id: EventTransactionId,
    options: &ResolvedTestOptions,
    commit: &Commit,
) -> eyre::Result<TestOutput> {
    let ResolvedTestOptions {
        command: _,
        strategy,
        verbosity,
    } = options;

    let (effects, progress) = effects.start_operation(OperationType::RunTestOnCommit(Arc::new(
        effects
            .get_glyphs()
            .render(commit.friendly_describe(effects.get_glyphs())?)?,
    )));

    let test_output = match make_test_files(repo, commit, options)? {
        TestFilesResult::Cached(test_output) => test_output,
        TestFilesResult::NotCached(test_files) => {
            match prepare_working_directory(git_run_info, repo, event_tx_id, commit, *strategy)? {
                None => {
                    let TestFiles {
                        lock_file: _, // Drop lock.
                        result_path,
                        result_file: _,
                        stdout_path,
                        stdout_file: _,
                        stderr_path,
                        stderr_file: _,
                    } = test_files;
                    TestOutput {
                        _result_path: result_path,
                        stdout_path,
                        stderr_path,
                        test_status: TestStatus::CheckoutFailed,
                    }
                }
                Some(working_directory) => test_commit(
                    repo,
                    test_files,
                    &working_directory,
                    shell_path,
                    options,
                    commit,
                )?,
            }
        }
    };

    let text = test_output.describe(&effects, commit, *verbosity)?;
    progress.notify_status(
        match test_output.test_status {
            TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_) => OperationIcon::Warning,

            TestStatus::TerminatedBySignal
            | TestStatus::Failed(_)
            | TestStatus::FailedCached(_) => OperationIcon::Failure,

            TestStatus::Passed | TestStatus::PassedCached => OperationIcon::Success,
        },
        effects.get_glyphs().render(text)?,
    );
    Ok(test_output)
}

#[derive(Debug)]
struct TestFiles {
    lock_file: LockFile,
    result_path: PathBuf,
    result_file: File,
    stdout_path: PathBuf,
    stdout_file: File,
    stderr_path: PathBuf,
    stderr_file: File,
}

#[derive(Debug)]
enum TestFilesResult {
    Cached(TestOutput),
    NotCached(TestFiles),
}

#[instrument]
fn make_test_files(
    repo: &Repo,
    commit: &Commit,
    options: &ResolvedTestOptions,
) -> eyre::Result<TestFilesResult> {
    let test_output_dir = repo.get_test_dir();
    let tree_oid = commit.get_tree()?.get_oid();
    let tree_dir = test_output_dir.join(tree_oid.to_string());
    std::fs::create_dir_all(&tree_dir)
        .wrap_err_with(|| format!("Creating tree directory {tree_dir:?}"))?;

    let command_dir = tree_dir.join(options.make_command_slug());
    std::fs::create_dir_all(&command_dir)
        .wrap_err_with(|| format!("Creating command directory {command_dir:?}"))?;

    let result_path = command_dir.join("result");
    let stdout_path = command_dir.join("stdout");
    let stderr_path = command_dir.join("stderr");
    let lock_path = command_dir.join("pid.lock");

    let mut lock_file =
        LockFile::open(&lock_path).wrap_err_with(|| format!("Opening lock file {lock_path:?}"))?;
    if !lock_file
        .try_lock_with_pid()
        .wrap_err_with(|| format!("Locking file {lock_path:?}"))?
    {
        return Ok(TestFilesResult::Cached(TestOutput {
            _result_path: result_path,
            stdout_path,
            stderr_path,
            test_status: TestStatus::AlreadyInProgress,
        }));
    }

    // Try to create the exit code file atomically.
    let result_file = match File::options()
        .create_new(true)
        .write(true)
        .open(&result_path)
    {
        Ok(result_file) => result_file,
        Err(_) => {
            let test_status = match std::fs::read_to_string(&result_path) {
                Err(err) => TestStatus::ReadCacheFailed(err.to_string()),
                Ok(contents) => match serde_json::from_str(&contents) {
                    Err(err) => TestStatus::ReadCacheFailed(err.to_string()),
                    Ok(TestStatus::Passed) => TestStatus::PassedCached,
                    Ok(TestStatus::Failed(exit_code)) => TestStatus::FailedCached(exit_code),
                    Ok(
                        test_result @ (TestStatus::AlreadyInProgress
                        | TestStatus::CheckoutFailed
                        | TestStatus::PassedCached
                        | TestStatus::FailedCached(_)
                        | TestStatus::ReadCacheFailed(_)
                        | TestStatus::SpawnTestFailed(_)
                        | TestStatus::TerminatedBySignal),
                    ) => TestStatus::ReadCacheFailed(format!(
                        "Unexpected cached test result: {test_result:?}"
                    )),
                },
            };
            return Ok(TestFilesResult::Cached(TestOutput {
                _result_path: result_path,
                stdout_path,
                stderr_path,
                test_status,
            }));
        }
    };

    let stdout_file = File::create(&stdout_path)
        .wrap_err_with(|| format!("Opening stdout file {stdout_path:?}"))?;
    let stderr_file = File::create(&stderr_path)
        .wrap_err_with(|| format!("Opening stderr file {stderr_path:?}"))?;
    Ok(TestFilesResult::NotCached(TestFiles {
        lock_file,
        result_path,
        result_file,
        stdout_path,
        stdout_file,
        stderr_path,
        stderr_file,
    }))
}

#[instrument]
fn prepare_working_directory(
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_tx_id: EventTransactionId,
    commit: &Commit,
    strategy: TestExecutionStrategy,
) -> eyre::Result<Option<PathBuf>> {
    match strategy {
        TestExecutionStrategy::WorkingCopy => {
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

        TestExecutionStrategy::Worktree => {
            let parent_dir = repo.get_test_dir().join("worktrees");
            std::fs::create_dir_all(&parent_dir)
                .wrap_err_with(|| format!("Creating worktree parent dir at {parent_dir:?}"))?;

            let worktree_dir = parent_dir.join("testing-worktree-1");
            let worktree_dir_str = match worktree_dir.to_str() {
                Some(worktree_dir) => worktree_dir,
                None => return Ok(None),
            };
            if !worktree_dir.exists() {
                let GitRunResult {
                    exit_code,
                    stdout: _,
                    stderr: _,
                } = git_run_info.run_silent(
                    repo,
                    Some(event_tx_id),
                    &["worktree", "add", worktree_dir_str, "--force", "--detach"],
                    Default::default(),
                )?;
                if !exit_code.is_success() {
                    return Ok(None);
                }
            }

            let GitRunResult {
                exit_code,
                stdout: _,
                stderr: _,
            } = git_run_info.run_silent(
                repo,
                Some(event_tx_id),
                &[
                    "-C",
                    worktree_dir_str,
                    "checkout",
                    "--force",
                    &commit.get_oid().to_string(),
                ],
                Default::default(),
            )?;
            if !exit_code.is_success() {
                return Ok(None);
            }
            Ok(Some(worktree_dir))
        }
    }
}

#[instrument]
fn test_commit(
    repo: &Repo,
    test_files: TestFiles,
    working_directory: &Path,
    shell_path: &Path,
    options: &ResolvedTestOptions,
    commit: &Commit,
) -> eyre::Result<TestOutput> {
    let TestFiles {
        lock_file: _lock_file, // Make sure not to drop lock.
        result_path,
        result_file,
        stdout_path,
        stdout_file,
        stderr_path,
        stderr_file,
    } = test_files;
    let exit_code = match Command::new(&shell_path)
        .arg("-c")
        .arg(&options.command)
        .current_dir(working_directory)
        .stdin(Stdio::null())
        .stdout(stdout_file)
        .stderr(stderr_file)
        .output()
    {
        Ok(output) => output.status.code(),
        Err(err) => {
            return Ok(TestOutput {
                _result_path: result_path,
                stdout_path,
                stderr_path,
                test_status: TestStatus::SpawnTestFailed(err.to_string()),
            });
        }
    };
    let exit_code = match exit_code {
        Some(exit_code) => exit_code,
        None => {
            return Ok(TestOutput {
                _result_path: result_path,
                stdout_path,
                stderr_path,
                test_status: TestStatus::TerminatedBySignal,
            });
        }
    };
    let test_status = match exit_code {
        0 => TestStatus::Passed,
        exit_code => TestStatus::Failed(exit_code),
    };

    serde_json::to_writer_pretty(result_file, &test_status)
        .wrap_err_with(|| format!("Writing test status {test_status:?} to {result_path:?}"))?;

    Ok(TestOutput {
        _result_path: result_path,
        stdout_path,
        stderr_path,
        test_status,
    })
}

#[instrument]
pub fn show(effects: &Effects, options: &RawTestOptions, revset: Revset) -> eyre::Result<ExitCode> {
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
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

    let options = match ResolvedTestOptions::resolve(effects, &repo, options)? {
        Ok(options) => options,
        Err(exit_code) => {
            return Ok(exit_code);
        }
    };

    let commit_set = match resolve_commits(effects, &repo, &mut dag, &[revset]) {
        Ok(mut commit_sets) => commit_sets.pop().unwrap(),
        Err(err) => {
            err.describe(effects)?;
            return Ok(ExitCode(1));
        }
    };

    let commits = sorted_commit_set(&repo, &dag, &commit_set)?;
    for commit in commits {
        let test_files = make_test_files(&repo, &commit, &options)?;
        match test_files {
            TestFilesResult::NotCached(_) => {
                writeln!(
                    effects.get_output_stream(),
                    "No cached test data for {}",
                    effects
                        .get_glyphs()
                        .render(commit.friendly_describe(effects.get_glyphs())?)?
                )?;
            }
            TestFilesResult::Cached(test_output) => {
                write!(
                    effects.get_output_stream(),
                    "{}",
                    effects.get_glyphs().render(test_output.describe(
                        effects,
                        &commit,
                        options.verbosity
                    )?)?,
                )?;
            }
        }
    }

    if get_hint_enabled(&repo, Hint::TestShowVerbose)? {
        match options.verbosity {
            Verbosity::None => {
                writeln!(
                    effects.get_output_stream(),
                    "{}: To see more detailed output, re-run with -v/--verbose.",
                    effects.get_glyphs().render(get_hint_string())?,
                )?;
                print_hint_suppression_notice(effects, Hint::TestShowVerbose)?;
            }
            Verbosity::PartialOutput => {
                writeln!(
                    effects.get_output_stream(),
                    "{}: To see more detailed output, re-run with -vv/--verbose --verbose.",
                    effects.get_glyphs().render(get_hint_string())?,
                )?;
                print_hint_suppression_notice(effects, Hint::TestShowVerbose)?;
            }
            Verbosity::FullOutput => {}
        }
    }

    Ok(ExitCode(0))
}

#[instrument]
pub fn clean(effects: &Effects, revset: Revset) -> eyre::Result<ExitCode> {
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
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

    let test_dir = repo.get_test_dir();
    if !test_dir.exists() {
        writeln!(
            effects.get_output_stream(),
            "No cached test results to clean."
        )?;
    }

    let mut num_cleaned_commits = 0;
    for commit in sorted_commit_set(&repo, &dag, &commit_set)? {
        let tree_oid = commit.get_tree()?.get_oid();
        let tree_dir = test_dir.join(tree_oid.to_string());
        if tree_dir.exists() {
            writeln!(
                effects.get_output_stream(),
                "Cleaning results for {}",
                effects
                    .get_glyphs()
                    .render(commit.friendly_describe(effects.get_glyphs())?)?,
            )?;
            std::fs::remove_dir_all(&tree_dir)
                .with_context(|| format!("Cleaning test dir: {tree_dir:?}"))?;
            num_cleaned_commits += 1;
        } else {
            writeln!(
                effects.get_output_stream(),
                "Nothing to clean for {}",
                effects
                    .get_glyphs()
                    .render(commit.friendly_describe(effects.get_glyphs())?)?,
            )?;
        }
    }
    writeln!(
        effects.get_output_stream(),
        "Cleaned {}.",
        Pluralize {
            determiner: None,
            amount: num_cleaned_commits,
            unit: ("cached test result", "cached test results")
        }
    )?;
    Ok(ExitCode(0))
}
