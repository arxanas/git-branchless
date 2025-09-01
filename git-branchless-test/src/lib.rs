//! Run a user-provided command on each of a set of provided commits. This is
//! useful to run checks on all commits in the current stack with caching,
//! parallelization, out-of-tree execution, etc.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_conditions)]

mod worker;

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::SystemTime;

use bstr::ByteSlice;
use clap::ValueEnum;
use crossbeam::channel::{Receiver, RecvError};
use cursive::theme::{BaseColor, Effect, Style};
use cursive::utils::markup::StyledString;

use eyre::WrapErr;
use fslock::LockFile;
use git_branchless_invoke::CommandContext;
use indexmap::IndexMap;
use itertools::Itertools;
use lazy_static::lazy_static;
use lib::core::check_out::CheckOutCommitOptions;
use lib::core::config::{
    get_hint_enabled, get_hint_string, get_restack_preserve_timestamps,
    print_hint_suppression_notice, Hint,
};
use lib::core::dag::{sorted_commit_set, CommitSet, Dag};
use lib::core::effects::{icons, Effects, OperationIcon, OperationType};
use lib::core::eventlog::{
    EventLogDb, EventReplayer, EventTransactionId, BRANCHLESS_TRANSACTION_ID_ENV_VAR,
};
use lib::core::formatting::{Glyphs, Pluralize, StyledStringBuilder};
use lib::core::repo_ext::RepoExt;
use lib::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanOptions, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    RebaseCommand, RebasePlan, RebasePlanBuilder, RebasePlanPermissions, RepoResource,
};
use lib::git::{
    get_latest_test_command_path, get_test_locks_dir, get_test_tree_dir, get_test_worktrees_dir,
    make_test_command_slug, Commit, ConfigRead, GitRunInfo, GitRunResult, MaybeZeroOid, NonZeroOid,
    Repo, SerializedNonZeroOid, SerializedTestResult, TestCommand, WorkingCopyChangesType,
    TEST_ABORT_EXIT_CODE, TEST_INDETERMINATE_EXIT_CODE, TEST_SUCCESS_EXIT_CODE,
};
use lib::try_exit_code;
use lib::util::{get_sh, ExitCode, EyreExitOr};
use rayon::ThreadPoolBuilder;
use scm_bisect::basic::{BasicSourceControlGraph, BasicStrategy, BasicStrategyKind};
use scm_bisect::search;
use tempfile::TempDir;
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

use git_branchless_opts::{
    MoveOptions, ResolveRevsetOptions, Revset, TestArgs, TestExecutionStrategy, TestSearchStrategy,
    TestSubcommand,
};
use git_branchless_revset::resolve_commits;

use crate::worker::{worker, JobResult, WorkQueue, WorkerId};

lazy_static! {
    static ref STYLE_SUCCESS: Style =
        Style::merge(&[BaseColor::Green.light().into(), Effect::Bold.into()]);
    static ref STYLE_FAILURE: Style =
        Style::merge(&[BaseColor::Red.light().into(), Effect::Bold.into()]);
    static ref STYLE_SKIPPED: Style =
        Style::merge(&[BaseColor::Yellow.light().into(), Effect::Bold.into()]);
}

/// How verbose of output to produce.
#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum Verbosity {
    /// Do not include test output at all.
    None,

    /// Include truncated test output.
    PartialOutput,

    /// Include the full test output.
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

/// The options for testing before they've assumed default values or been
/// validated.
#[derive(Debug)]
pub struct RawTestOptions {
    /// The command to execute, if any.
    pub exec: Option<String>,

    /// The command alias to execute, if any.
    pub command: Option<String>,

    /// Whether or not to execute as a "dry-run", i.e. don't rewrite any commits
    /// if `true`.
    pub dry_run: bool,

    /// The execution strategy to use.
    pub strategy: Option<TestExecutionStrategy>,

    /// Search for the first commit that fails the test command, rather than
    /// running on all commits.
    pub search: Option<TestSearchStrategy>,

    /// Shorthand for the binary search strategy.
    pub bisect: bool,

    /// Don't read or write to the cache when executing the test commands.
    pub no_cache: bool,

    /// Whether to run interactively.
    pub interactive: bool,

    /// The number of jobs to run in parallel.
    pub jobs: Option<usize>,

    /// The requested verbosity of the test output.
    pub verbosity: Verbosity,

    /// Whether to amend commits with the changes produced by the executed
    /// command.
    pub apply_fixes: bool,
}

fn resolve_test_command_alias(
    effects: &Effects,
    repo: &Repo,
    alias: Option<&str>,
) -> EyreExitOr<String> {
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

/// The values from a `RawTestOptions` but with defaults provided. See
/// [`RawTestOptions`] for details on these options.
#[allow(missing_docs)]
#[derive(Debug)]
pub struct ResolvedTestOptions {
    pub command: TestCommand,
    pub execution_strategy: TestExecutionStrategy,
    pub search_strategy: Option<TestSearchStrategy>,
    pub is_dry_run: bool,
    pub use_cache: bool,
    pub is_interactive: bool,
    pub num_jobs: usize,
    pub verbosity: Verbosity,
    pub fix_options: Option<(ExecuteRebasePlanOptions, RebasePlanPermissions)>,
}

impl ResolvedTestOptions {
    /// Resolve any missing test options with their defaults from the environment.
    pub fn resolve(
        now: SystemTime,
        effects: &Effects,
        dag: &Dag,
        repo: &Repo,
        event_tx_id: EventTransactionId,
        commits: &CommitSet,
        move_options: Option<&MoveOptions>,
        options: &RawTestOptions,
    ) -> EyreExitOr<Self> {
        let config = repo.get_readonly_config()?;
        let RawTestOptions {
            exec: command,
            command: command_alias,
            dry_run,
            strategy,
            search,
            bisect,
            no_cache,
            interactive,
            jobs,
            verbosity,
            apply_fixes,
        } = options;
        let resolved_command = match (command, command_alias) {
            (Some(command), None) => command.to_owned(),
            (None, None) => match (interactive, std::env::var("SHELL")) {
                (true, Ok(shell)) => shell,
                _ => match resolve_test_command_alias(effects, repo, None)? {
                    Ok(command) => command,
                    Err(exit_code) => {
                        return Ok(Err(exit_code));
                    }
                },
            },
            (None, Some(command_alias)) => {
                match resolve_test_command_alias(effects, repo, Some(command_alias))? {
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
        let configured_execution_strategy = match strategy {
            Some(strategy) => *strategy,
            None => {
                let strategy_config_key = "branchless.test.strategy";
                let strategy: Option<String> = config.get(strategy_config_key)?;
                match strategy {
                    None => TestExecutionStrategy::WorkingCopy,
                    Some(strategy) => {
                        match TestExecutionStrategy::from_str(&strategy, true) {
                            Ok(strategy) => strategy,
                            Err(_) => {
                                writeln!(effects.get_output_stream(), "Invalid value for config value {strategy_config_key}: {strategy}")?;
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
                        }
                    }
                }
            }
        };

        let jobs_config_key = "branchless.test.jobs";
        let configured_jobs: Option<i32> = config.get(jobs_config_key)?;
        let configured_jobs = match configured_jobs {
            None => None,
            Some(configured_jobs) => match usize::try_from(configured_jobs) {
                Ok(configured_jobs) => Some(configured_jobs),
                Err(err) => {
                    writeln!(
                        effects.get_output_stream(),
                        "Invalid value for config value for {jobs_config_key} ({configured_jobs}): {err}"
                    )?;
                    return Ok(Err(ExitCode(1)));
                }
            },
        };
        let (resolved_num_jobs, resolved_execution_strategy, resolved_interactive) = match jobs {
            None => match (strategy, *interactive) {
                (Some(TestExecutionStrategy::WorkingCopy), interactive) => {
                    (1, TestExecutionStrategy::WorkingCopy, interactive)
                }
                (Some(TestExecutionStrategy::Worktree), true) => {
                    (1, TestExecutionStrategy::Worktree, true)
                }
                (Some(TestExecutionStrategy::Worktree), false) => (
                    configured_jobs.unwrap_or(1),
                    TestExecutionStrategy::Worktree,
                    false,
                ),
                (None, true) => (1, configured_execution_strategy, true),
                (None, false) => (
                    configured_jobs.unwrap_or(1),
                    configured_execution_strategy,
                    false,
                ),
            },
            Some(1) => (1, configured_execution_strategy, *interactive),
            Some(jobs) => {
                if *interactive {
                    writeln!(
                        effects.get_output_stream(),
                        "\
The --jobs option cannot be used with the --interactive option."
                    )?;
                    return Ok(Err(ExitCode(1)));
                }
                // NB: match on the strategy passed on the command-line here, not the resolved strategy.
                match strategy {
                    None | Some(TestExecutionStrategy::Worktree) => {
                        (*jobs, TestExecutionStrategy::Worktree, false)
                    }
                    Some(TestExecutionStrategy::WorkingCopy) => {
                        writeln!(
                            effects.get_output_stream(),
                            "\
The --jobs option can only be used with --strategy worktree, but --strategy working-copy was provided instead."
                        )?;
                        return Ok(Err(ExitCode(1)));
                    }
                }
            }
        };

        if resolved_interactive != *interactive {
            writeln!(effects.get_output_stream(),
            "\
BUG: Expected resolved_interactive ({resolved_interactive:?}) to match interactive ({interactive:?}). If it doesn't match, then multiple interactive jobs might inadvertently be launched in parallel."
            )?;
            return Ok(Err(ExitCode(1)));
        }

        let resolved_num_jobs = if resolved_num_jobs == 0 {
            num_cpus::get_physical()
        } else {
            resolved_num_jobs
        };
        assert!(resolved_num_jobs > 0);

        let fix_options = if *apply_fixes {
            let move_options = match move_options {
                Some(move_options) => move_options,
                None => {
                    writeln!(effects.get_output_stream(), "BUG: fixes were requested to be applied, but no `BuildRebasePlanOptions` were provided.")?;
                    return Ok(Err(ExitCode(1)));
                }
            };
            let MoveOptions {
                force_rewrite_public_commits,
                force_in_memory: _,
                force_on_disk,
                detect_duplicate_commits_via_patch_id,
                resolve_merge_conflicts,
                dump_rebase_constraints,
                dump_rebase_plan,
            } = move_options;

            let force_in_memory = true;
            if *force_on_disk {
                writeln!(
                    effects.get_output_stream(),
                    "The --on-disk option cannot be provided for fixes. Use the --in-memory option instead."
                )?;
                return Ok(Err(ExitCode(1)));
            }

            let build_options = BuildRebasePlanOptions {
                force_rewrite_public_commits: *force_rewrite_public_commits,
                dump_rebase_constraints: *dump_rebase_constraints,
                dump_rebase_plan: *dump_rebase_plan,
                detect_duplicate_commits_via_patch_id: *detect_duplicate_commits_via_patch_id,
            };
            let execute_options = ExecuteRebasePlanOptions {
                now,
                event_tx_id,
                preserve_timestamps: get_restack_preserve_timestamps(repo)?,
                force_in_memory,
                force_on_disk: *force_on_disk,
                resolve_merge_conflicts: *resolve_merge_conflicts,
                check_out_commit_options: CheckOutCommitOptions {
                    render_smartlog: false,
                    ..Default::default()
                },
            };
            let permissions =
                match RebasePlanPermissions::verify_rewrite_set(dag, build_options, commits)? {
                    Ok(permissions) => permissions,
                    Err(err) => {
                        err.describe(effects, repo, dag)?;
                        return Ok(Err(ExitCode(1)));
                    }
                };
            Some((execute_options, permissions))
        } else {
            None
        };

        let resolved_search_strategy = if *bisect {
            Some(TestSearchStrategy::Binary)
        } else {
            *search
        };

        let resolved_test_options = ResolvedTestOptions {
            command: TestCommand::String(resolved_command),
            execution_strategy: resolved_execution_strategy,
            search_strategy: resolved_search_strategy,
            use_cache: !no_cache,
            is_dry_run: *dry_run,
            is_interactive: resolved_interactive,
            num_jobs: resolved_num_jobs,
            verbosity: *verbosity,
            fix_options,
        };
        debug!(?resolved_test_options, "Resolved test options");
        Ok(Ok(resolved_test_options))
    }

    fn make_command_slug(&self) -> String {
        make_test_command_slug(self.command.to_string())
    }
}

/// `test` command.
#[instrument]
pub fn command_main(ctx: CommandContext, args: TestArgs) -> EyreExitOr<()> {
    let CommandContext {
        effects,
        git_run_info,
    } = ctx;
    let TestArgs { subcommand } = args;
    match subcommand {
        TestSubcommand::Clean {
            revset,
            resolve_revset_options,
        } => subcommand_clean(&effects, revset, &resolve_revset_options),

        TestSubcommand::Run {
            exec: command,
            command: command_alias,
            revset,
            resolve_revset_options,
            verbosity,
            strategy,
            search,
            bisect,
            no_cache,
            interactive,
            jobs,
        } => subcommand_run(
            &effects,
            &git_run_info,
            &RawTestOptions {
                exec: command,
                command: command_alias,
                dry_run: false,
                strategy,
                search,
                bisect,
                no_cache,
                interactive,
                jobs,
                verbosity: Verbosity::from(verbosity),
                apply_fixes: false,
            },
            revset,
            &resolve_revset_options,
            None,
        ),

        TestSubcommand::Show {
            exec: command,
            command: command_alias,
            revset,
            resolve_revset_options,
            verbosity,
        } => subcommand_show(
            &effects,
            &RawTestOptions {
                exec: command,
                command: command_alias,
                dry_run: false,
                strategy: None,
                search: None,
                bisect: false,
                no_cache: false,
                interactive: false,
                jobs: None,
                verbosity: Verbosity::from(verbosity),
                apply_fixes: false,
            },
            revset,
            &resolve_revset_options,
        ),

        TestSubcommand::Fix {
            exec: command,
            command: command_alias,
            dry_run,
            revset,
            resolve_revset_options,
            verbosity,
            strategy,
            no_cache,
            jobs,
            move_options,
        } => subcommand_run(
            &effects,
            &git_run_info,
            &RawTestOptions {
                exec: command,
                command: command_alias,
                dry_run,
                strategy,
                search: None,
                bisect: false,
                no_cache,
                interactive: false,
                jobs,
                verbosity: Verbosity::from(verbosity),
                apply_fixes: true,
            },
            revset,
            &resolve_revset_options,
            Some(&move_options),
        ),
    }
}

/// Run the command provided in `options` on each of the commits in `revset`.
#[instrument]
fn subcommand_run(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    options: &RawTestOptions,
    revset: Revset,
    resolve_revset_options: &ResolveRevsetOptions,
    move_options: Option<&MoveOptions>,
) -> EyreExitOr<()> {
    let now = SystemTime::now();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "test run")?;
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

    let commit_set = match resolve_commits(
        effects,
        &repo,
        &mut dag,
        std::slice::from_ref(&revset),
        resolve_revset_options,
    ) {
        Ok(mut commit_sets) => commit_sets.pop().unwrap(),
        Err(err) => {
            err.describe(effects)?;
            return Ok(Err(ExitCode(1)));
        }
    };

    let options = try_exit_code!(ResolvedTestOptions::resolve(
        now,
        effects,
        &dag,
        &repo,
        event_tx_id,
        &commit_set,
        move_options,
        options,
    )?);

    let commits = sorted_commit_set(&repo, &dag, &commit_set)?;
    let test_results = try_exit_code!(run_tests(
        now,
        effects,
        git_run_info,
        &dag,
        &repo,
        &event_log_db,
        &revset,
        &commits,
        &options,
    )?);

    try_exit_code!(print_summary(
        effects,
        &dag,
        &repo,
        &revset,
        &options.command,
        &test_results,
        options.search_strategy.is_some(),
        options.fix_options.is_some(),
        &options.verbosity,
    )?);

    if let Some((execute_options, permissions)) = &options.fix_options {
        try_exit_code!(apply_fixes(
            effects,
            git_run_info,
            &mut dag,
            &repo,
            &event_log_db,
            execute_options,
            permissions.clone(),
            options.is_dry_run,
            &options.command,
            &test_results,
        )?);
    }

    Ok(Ok(()))
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
) -> EyreExitOr<AbortTrap> {
    match strategy {
        TestExecutionStrategy::Worktree => return Ok(Ok(AbortTrap { is_active: false })),
        TestExecutionStrategy::WorkingCopy => {}
    }

    if let Some(operation_type) = repo.get_current_operation_type() {
        writeln!(
            effects.get_output_stream(),
            "A {operation_type} operation is already in progress."
        )?;
        writeln!(
            effects.get_output_stream(),
            "Run git {operation_type} --continue or git {operation_type} --abort to resolve it and proceed."
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
            check_out_commit_options: CheckOutCommitOptions {
                render_smartlog: false,
                ..Default::default()
            },
        },
    )? {
        ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ } => {
            // Do nothing.
        }
        ExecuteRebasePlanResult::DeclinedToMerge { failed_merge_info } => {
            writeln!(
                effects.get_output_stream(),
                "BUG: Encountered unexpected merge failure: {failed_merge_info:?}"
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
) -> EyreExitOr<()> {
    let AbortTrap { is_active } = abort_trap;
    if is_active {
        try_exit_code!(git_run_info.run(effects, Some(event_tx_id), &["rebase", "--abort"])?);
    }
    Ok(Ok(()))
}

/// The result of running a test.
#[derive(Debug)]
pub struct TestOutput {
    /// Only used for `--no-cache` invocations, to ensure that we don't clobber
    /// an existing cached entry.
    pub temp_dir: Option<TempDir>,

    /// The path to the file containing the serialized test result information
    /// (such as the exit code).
    pub result_path: PathBuf,

    /// The path to the file containing the stdout of the test command.
    pub stdout_path: PathBuf,

    /// The path to the file containing the stderr of the test command.
    pub stderr_path: PathBuf,

    /// The resulting status of the test.
    pub test_status: TestStatus,
}

/// The possible results of attempting to run a test.
#[derive(Clone, Debug)]
pub enum TestStatus {
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

    /// The test command indicated that the commit should be skipped for testing.
    Indeterminate {
        /// The exit code of the command.
        exit_code: i32,
    },

    /// The test command indicated that the process should be aborted entirely.
    Abort {
        /// The exit code of the command.
        exit_code: i32,
    },

    /// The test failed and returned the provided (non-zero) exit code.
    Failed {
        /// Whether or not the result was cached (indicating that we didn't
        /// actually re-run the test).
        cached: bool,

        /// The exit code of the process.
        exit_code: i32,

        /// Whether the test was run interactively (the user executed the
        /// command via `--interactive`).
        interactive: bool,
    },

    /// The test passed and returned a successful exit code.
    Passed {
        /// Whether or not the result was cached (indicating that we didn't
        /// actually re-run the test).
        cached: bool,

        /// Information about the working copy state after running the test command.
        fix_info: FixInfo,

        /// Whether the test was run interactively (the user executed the
        /// command via `--interactive`).
        interactive: bool,
    },
}

/// Information about the working copy state after running the test command.
#[derive(Clone, Debug)]
pub struct FixInfo {
    /// The resulting commit which was checked out as `HEAD`. This is usually
    /// the same commit as was being tested, but the test command could amend or
    /// switch to a different commit.
    pub head_commit_oid: Option<NonZeroOid>,

    /// The resulting contents of the working copy after the command was
    /// executed (if taking a working copy snapshot succeeded and there were no
    /// merge conflicts, etc.).
    pub snapshot_tree_oid: Option<NonZeroOid>,
}

impl TestStatus {
    #[instrument]
    fn get_icon(&self) -> &'static str {
        match self {
            TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_)
            | TestStatus::TerminatedBySignal
            | TestStatus::Indeterminate { .. } => icons::EXCLAMATION,
            TestStatus::Failed { .. } | TestStatus::Abort { .. } => icons::CROSS,
            TestStatus::Passed { .. } => icons::CHECKMARK,
        }
    }

    #[instrument]
    fn get_style(&self) -> Style {
        match self {
            TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_)
            | TestStatus::TerminatedBySignal
            | TestStatus::Indeterminate { .. } => *STYLE_SKIPPED,
            TestStatus::Failed { .. } | TestStatus::Abort { .. } => *STYLE_FAILURE,
            TestStatus::Passed { .. } => *STYLE_SUCCESS,
        }
    }

    /// Produce a friendly description of the test status.
    #[instrument]
    pub fn describe(
        &self,
        glyphs: &Glyphs,
        commit: &Commit,
        apply_fixes: bool,
    ) -> eyre::Result<StyledString> {
        let description = match self {
            TestStatus::CheckoutFailed => StyledStringBuilder::new()
                .append_styled("Failed to check out: ", self.get_style())
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::SpawnTestFailed(err) => StyledStringBuilder::new()
                .append_styled(
                    format!("Failed to spawn command: {err}: "),
                    self.get_style(),
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::TerminatedBySignal => StyledStringBuilder::new()
                .append_styled("Command terminated by signal: ", self.get_style())
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::AlreadyInProgress => StyledStringBuilder::new()
                .append_styled("Command already in progress? ", self.get_style())
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::ReadCacheFailed(_) => StyledStringBuilder::new()
                .append_styled("Could not read cached command result: ", self.get_style())
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::Indeterminate { exit_code } => StyledStringBuilder::new()
                .append_styled(
                    format!("Exit code indicated to skip this commit (exit code {exit_code}): "),
                    self.get_style(),
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::Abort { exit_code } => StyledStringBuilder::new()
                .append_styled(
                    format!("Exit code indicated to abort command (exit code {exit_code}): "),
                    self.get_style(),
                )
                .append(commit.friendly_describe(glyphs)?)
                .build(),

            TestStatus::Failed {
                cached,
                interactive,
                exit_code,
            } => {
                let mut descriptors = Vec::new();
                if *cached {
                    descriptors.push("cached".to_string());
                }
                descriptors.push(format!("exit code {exit_code}"));
                if *interactive {
                    descriptors.push("interactive".to_string());
                }
                let descriptors = descriptors.join(", ");
                StyledStringBuilder::new()
                    .append_styled(format!("Failed ({descriptors}): "), self.get_style())
                    .append(commit.friendly_describe(glyphs)?)
                    .build()
            }

            TestStatus::Passed {
                cached,
                interactive,
                fix_info:
                    FixInfo {
                        head_commit_oid: _,
                        snapshot_tree_oid,
                    },
            } => {
                let mut descriptors = Vec::new();
                if *cached {
                    descriptors.push("cached".to_string());
                }
                match (snapshot_tree_oid, commit.get_tree_oid()) {
                    (Some(snapshot_tree_oid), MaybeZeroOid::NonZero(original_tree_oid)) => {
                        if *snapshot_tree_oid != original_tree_oid {
                            descriptors.push(if apply_fixes {
                                "fixed".to_string()
                            } else {
                                "fixable".to_string()
                            });
                        }
                    }
                    (None, _) | (_, MaybeZeroOid::Zero) => {}
                }
                if *interactive {
                    descriptors.push("interactive".to_string());
                }
                let descriptors = if descriptors.is_empty() {
                    "".to_string()
                } else {
                    format!(" ({})", descriptors.join(", "))
                };
                StyledStringBuilder::new()
                    .append_styled(format!("Passed{descriptors}: "), self.get_style())
                    .append(commit.friendly_describe(glyphs)?)
                    .build()
            }
        };
        Ok(description)
    }
}

/// An error produced when testing is aborted due to a certain commit.
#[derive(Debug)]
pub struct TestingAbortedError {
    /// The commit which aborted testing.
    pub commit_oid: NonZeroOid,

    /// The exit code of the test command when run on that commit.
    pub exit_code: i32,
}

impl TestOutput {
    #[instrument]
    fn describe(
        &self,
        effects: &Effects,
        commit: &Commit,
        apply_fixes: bool,
        verbosity: Verbosity,
    ) -> eyre::Result<StyledString> {
        let description = StyledStringBuilder::new()
            .append_styled(self.test_status.get_icon(), self.test_status.get_style())
            .append_plain(" ")
            .append(
                self.test_status
                    .describe(effects.get_glyphs(), commit, apply_fixes)?,
            )
            .build();

        if verbosity == Verbosity::None {
            return Ok(StyledStringBuilder::from_lines(vec![description]));
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

        let interactive = match self.test_status {
            TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::TerminatedBySignal
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_)
            | TestStatus::Indeterminate { .. }
            | TestStatus::Abort { .. } => false,
            TestStatus::Failed { interactive, .. } | TestStatus::Passed { interactive, .. } => {
                interactive
            }
        };

        let stdout_lines = {
            let mut lines = Vec::new();
            if !interactive {
                lines.push(
                    StyledStringBuilder::new()
                        .append_styled("Stdout: ", Effect::Bold)
                        .append_plain(self.stdout_path.to_string_lossy())
                        .build(),
                );
                lines.extend(abbreviate_lines(&self.stdout_path, verbosity));
            }
            lines
        };
        let stderr_lines = {
            let mut lines = Vec::new();
            if !interactive {
                lines.push(
                    StyledStringBuilder::new()
                        .append_styled("Stderr: ", Effect::Bold)
                        .append_plain(self.stderr_path.to_string_lossy())
                        .build(),
                );
                lines.extend(abbreviate_lines(&self.stderr_path, verbosity));
            }
            lines
        };

        Ok(StyledStringBuilder::from_lines(
            [
                &[description],
                stdout_lines.as_slice(),
                stderr_lines.as_slice(),
            ]
            .concat(),
        ))
    }
}

fn shell_escape(s: impl AsRef<str>) -> String {
    let s = s.as_ref();
    let mut escaped = String::new();
    escaped.push('"');
    for c in s.chars() {
        match c {
            '"' => escaped.push_str(r#"\""#),
            '\\' => escaped.push_str(r"\\\\"),
            c => escaped.push(c),
        }
    }
    escaped.push('"');
    escaped
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TestJob {
    commit_oid: NonZeroOid,
    operation_type: OperationType,
}

#[derive(Debug, Error)]
enum SearchGraphError {
    #[error(transparent)]
    Dag(#[from] eden_dag::Error),

    #[error(transparent)]
    Other(#[from] eyre::Error),
}

#[derive(Debug)]
struct SearchGraph<'a> {
    dag: &'a Dag,
    commit_set: CommitSet,
}

impl BasicSourceControlGraph for SearchGraph<'_> {
    type Node = NonZeroOid;
    type Error = SearchGraphError;

    #[instrument]
    fn ancestors(&self, node: Self::Node) -> Result<HashSet<Self::Node>, Self::Error> {
        let ancestors = self.dag.query_ancestors(CommitSet::from(node))?;
        let ancestors = ancestors.intersection(&self.commit_set);
        let ancestors = self.dag.commit_set_to_vec(&ancestors)?;
        Ok(ancestors.into_iter().collect())
    }

    #[instrument]
    fn descendants(&self, node: Self::Node) -> Result<HashSet<Self::Node>, Self::Error> {
        let descendants = self.dag.query_descendants(CommitSet::from(node))?;
        let descendants = descendants.intersection(&self.commit_set);
        let descendants = self.dag.commit_set_to_vec(&descendants)?;
        Ok(descendants.into_iter().collect())
    }
}

/// The results of running all tests.
#[derive(Debug)]
pub struct TestResults {
    /// If a search strategy was provided, the calculated bounds on the input
    /// commit set.
    pub search_bounds: search::Bounds<NonZeroOid>,

    /// The test output for each commit.
    pub test_outputs: IndexMap<NonZeroOid, TestOutput>,

    /// If testing was aborted, the corresponding error.
    pub testing_aborted_error: Option<TestingAbortedError>,
}

/// Run tests on the provided set of commits.
#[instrument]
pub fn run_tests<'a>(
    now: SystemTime,
    effects: &Effects,
    git_run_info: &GitRunInfo,
    dag: &Dag,
    repo: &Repo,
    event_log_db: &EventLogDb,
    revset: &Revset,
    commits: &[Commit],
    options: &ResolvedTestOptions,
) -> EyreExitOr<TestResults> {
    let event_tx_id = EventTransactionId::Suppressed;
    let abort_trap = match set_abort_trap(
        now,
        effects,
        git_run_info,
        repo,
        event_log_db,
        event_tx_id,
        options.execution_strategy,
    )? {
        Ok(abort_trap) => abort_trap,
        Err(exit_code) => return Ok(Err(exit_code)),
    };
    let test_results: Result<_, _> = {
        let effects = if options.is_interactive {
            effects.suppress()
        } else {
            effects.clone()
        };
        run_tests_inner(
            &effects,
            git_run_info,
            dag,
            repo,
            event_log_db,
            event_tx_id,
            revset,
            commits,
            options,
        )
    };

    try_exit_code!(clear_abort_trap(
        effects,
        git_run_info,
        event_tx_id,
        abort_trap
    )?);
    test_results
}

#[instrument]
fn run_tests_inner<'a>(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    dag: &Dag,
    repo: &Repo,
    event_log_db: &EventLogDb,
    event_tx_id: EventTransactionId,
    revset: &Revset,
    commits: &[Commit],
    options: &ResolvedTestOptions,
) -> EyreExitOr<TestResults> {
    let ResolvedTestOptions {
        command,
        execution_strategy,
        search_strategy,
        use_cache: _,      // Used only in `make_test_files`.
        is_dry_run: _,     // Used only in `apply_fixes`.
        is_interactive: _, // Used in `test_commit`.
        num_jobs,
        verbosity: _,   // Verbosity used by caller to print results.
        fix_options: _, // Whether to apply fixes is checked by `test_commit`, after the working directory is set up.
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
            return Ok(Err(ExitCode(1)));
        }
    };

    if let Some(strategy_value) = execution_strategy.to_possible_value() {
        writeln!(
            effects.get_output_stream(),
            "Using command execution strategy: {}",
            effects.get_glyphs().render(
                StyledStringBuilder::new()
                    .append_styled(strategy_value.get_name(), Effect::Bold)
                    .build()
            )?,
        )?;
    }

    if let Some(strategy_value) = search_strategy.and_then(|opt| opt.to_possible_value()) {
        writeln!(
            effects.get_output_stream(),
            "Using test search strategy: {}",
            effects.get_glyphs().render(
                StyledStringBuilder::new()
                    .append_styled(strategy_value.get_name(), Effect::Bold)
                    .build()
            )?,
        )?;
    }
    let search_strategy = match search_strategy {
        None => None,
        Some(TestSearchStrategy::Linear) => Some(BasicStrategyKind::Linear),
        Some(TestSearchStrategy::Reverse) => Some(BasicStrategyKind::LinearReverse),
        Some(TestSearchStrategy::Binary) => Some(BasicStrategyKind::Binary),
    };
    let search_strategy = search_strategy.map(BasicStrategy::new);

    let latest_test_command_path = get_latest_test_command_path(repo)?;
    if let Some(parent) = latest_test_command_path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            warn!(
                ?err,
                ?latest_test_command_path,
                "Failed to create containing directory for latest test command"
            );
        }
    }
    if let Err(err) = std::fs::write(&latest_test_command_path, command.to_string()) {
        warn!(
            ?err,
            ?latest_test_command_path,
            "Failed to write latest test command to disk"
        );
    }

    let EventLoopOutput {
        search,
        test_outputs: test_outputs_unordered,
        testing_aborted_error,
    } = {
        let (effects, progress) =
            effects.start_operation(OperationType::RunTests(Arc::new(command.to_string())));
        progress.notify_progress(0, commits.len());
        let commit_jobs = {
            let mut results = IndexMap::new();
            for commit in commits {
                // Create the progress entries in the multiprogress meter without starting them.
                // They'll be resumed later in the loop below.
                let commit_description = effects
                    .get_glyphs()
                    .render(commit.friendly_describe(effects.get_glyphs())?)?;
                let operation_type =
                    OperationType::RunTestOnCommit(Arc::new(commit_description.clone()));
                let (_effects, progress) = effects.start_operation(operation_type.clone());
                progress.notify_status(
                    OperationIcon::InProgress,
                    format!("Waiting to run on {commit_description}"),
                );
                results.insert(
                    commit.get_oid(),
                    TestJob {
                        commit_oid: commit.get_oid(),
                        operation_type,
                    },
                );
            }
            results
        };

        let graph = SearchGraph {
            dag,
            commit_set: commits.iter().map(|c| c.get_oid()).collect(),
        };
        let search = search::Search::new(graph, commits.iter().map(|c| c.get_oid()));

        let work_queue = WorkQueue::new();
        let repo_dir = repo.get_path();
        crossbeam::thread::scope(|scope| -> eyre::Result<_> {
            let (result_tx, result_rx) = crossbeam::channel::unbounded();
            let workers: HashMap<WorkerId, crossbeam::thread::ScopedJoinHandle<()>> = {
                let mut result = HashMap::new();
                for worker_id in 1..=*num_jobs {
                    let effects = &effects;
                    let progress = &progress;
                    let shell_path = &shell_path;
                    let work_queue = work_queue.clone();
                    let result_tx = result_tx.clone();
                    let setup = move || -> eyre::Result<Repo> {
                        let repo = Repo::from_dir(repo_dir)?;
                        Ok(repo)
                    };
                    let f = move |job: TestJob, repo: &Repo| -> eyre::Result<TestOutput> {
                        let TestJob {
                            commit_oid,
                            operation_type,
                        } = job;
                        let commit = repo.find_commit_or_fail(commit_oid)?;
                        run_test(
                            effects,
                            operation_type,
                            git_run_info,
                            shell_path,
                            repo,
                            event_tx_id,
                            options,
                            worker_id,
                            &commit,
                        )
                    };
                    result.insert(
                        worker_id,
                        scope.spawn(move |_scope| {
                            worker(progress, worker_id, work_queue, result_tx, setup, f);
                            debug!("Exiting spawned thread closure");
                        }),
                    );
                }
                result
            };

            // We rely on `result_rx.recv()` returning `Err` once all the threads have exited
            // (whether that reason should be panicking or having finished all work). We have to
            // drop our local reference to ensure that we don't deadlock, since otherwise there
            // would still be a live receiver for the sender.
            drop(result_tx);

            let test_results = event_loop(
                commit_jobs,
                search,
                search_strategy.clone(),
                *num_jobs,
                work_queue.clone(),
                result_rx,
            );
            work_queue.close();
            let test_results = test_results?;

            if test_results.testing_aborted_error.is_none() && search_strategy.is_none() {
                debug!("Waiting for workers");
                progress.notify_status(OperationIcon::InProgress, "Waiting for workers");
                for (worker_id, worker) in workers {
                    worker
                        .join()
                        .map_err(|_err| eyre::eyre!("Waiting for worker {worker_id} to exit"))?;
                }
            }

            debug!("About to return from thread scope");
            Ok(test_results)
        })
        .map_err(|_| eyre::eyre!("Could not spawn workers"))?
        .wrap_err("Failed waiting on workers")?
    };
    debug!("Returned from thread scope");

    // The results may be returned in an arbitrary order if they were produced
    // in parallel, so recover the input order to produce deterministic output.
    let test_outputs_ordered: IndexMap<NonZeroOid, TestOutput> = {
        let mut test_outputs_unordered = test_outputs_unordered;
        let mut test_outputs_ordered = IndexMap::new();
        for commit_oid in commits.iter().map(|commit| commit.get_oid()) {
            match test_outputs_unordered.remove(&commit_oid) {
                Some(result) => {
                    test_outputs_ordered.insert(commit_oid, result);
                }
                None => {
                    if search_strategy.is_none() && testing_aborted_error.is_none() {
                        warn!(?commit_oid, "No result was returned for commit");
                    }
                }
            }
        }
        if !test_outputs_unordered.is_empty() {
            warn!(
                ?test_outputs_unordered,
                ?commits,
                "There were extra results for commits not appearing in the input list"
            );
        }
        test_outputs_ordered
    };

    Ok(Ok(TestResults {
        search_bounds: match search_strategy {
            None => Default::default(),
            Some(search_strategy) => search.search(&search_strategy)?.bounds,
        },
        test_outputs: test_outputs_ordered,
        testing_aborted_error,
    }))
}

struct EventLoopOutput<'a> {
    search: search::Search<SearchGraph<'a>>,
    test_outputs: HashMap<NonZeroOid, TestOutput>,
    testing_aborted_error: Option<TestingAbortedError>,
}

fn event_loop(
    commit_jobs: IndexMap<NonZeroOid, TestJob>,
    mut search: search::Search<SearchGraph>,
    search_strategy: Option<BasicStrategy>,
    num_jobs: usize,
    work_queue: WorkQueue<TestJob>,
    result_rx: Receiver<JobResult<TestJob, TestOutput>>,
) -> eyre::Result<EventLoopOutput> {
    #[derive(Debug)]
    enum ScheduledJob {
        Scheduled(TestJob),
        Complete(TestOutput),
    }
    let mut scheduled_jobs: HashMap<NonZeroOid, ScheduledJob> = Default::default();
    let mut testing_aborted_error = None;

    if search_strategy.is_none() {
        let jobs_to_schedule = commit_jobs
            .keys()
            .map(|commit_oid| commit_jobs[commit_oid].clone())
            .collect_vec();
        debug!(
            ?jobs_to_schedule,
            "Scheduling all jobs (since no search strategy was specified)"
        );
        for job in &jobs_to_schedule {
            scheduled_jobs.insert(job.commit_oid, ScheduledJob::Scheduled(job.clone()));
        }
        work_queue.set(jobs_to_schedule);
    }

    loop {
        if let Some(err) = &testing_aborted_error {
            debug!(?err, "Testing aborted");
            break;
        }

        if let Some(search_strategy) = &search_strategy {
            scheduled_jobs = scheduled_jobs
                .into_iter()
                .filter_map(|(commit_oid, scheduled_job)| match scheduled_job {
                    ScheduledJob::Scheduled(_) => None,
                    scheduled_job @ ScheduledJob::Complete(_) => Some((commit_oid, scheduled_job)),
                })
                .collect();

            let solution = search.search(search_strategy)?;
            let next_to_search: Vec<_> = solution
                .next_to_search
                .filter(|commit_oid| {
                    let commit_oid = match commit_oid {
                        Ok(commit_oid) => commit_oid,
                        Err(_) => {
                            // Collect `Err` below.
                            return true;
                        }
                    };

                    // At this point, `scheduled_jobs` should only contain
                    // completed jobs.
                    match scheduled_jobs.get(commit_oid) {
                        Some(ScheduledJob::Complete(_)) => false,
                        Some(ScheduledJob::Scheduled(_)) => {
                            warn!(
                                ?commit_oid,
                                "Left-over scheduled job; this should have already been filtered out."
                            );
                            true
                        }
                        None => true,
                    }
                })
                .take(num_jobs)
                .try_collect()?;
            if next_to_search.is_empty() {
                debug!("Search completed, exiting.");
                break;
            }
            let jobs_to_schedule = next_to_search
                .into_iter()
                .map(|commit_oid| commit_jobs[&commit_oid].clone())
                .collect_vec();
            debug!(
                ?search_strategy,
                ?jobs_to_schedule,
                "Jobs to schedule for search"
            );
            for job in &jobs_to_schedule {
                if let Some(previous_job) =
                    scheduled_jobs.insert(job.commit_oid, ScheduledJob::Scheduled(job.clone()))
                {
                    warn!(?job, ?previous_job, "Overwriting previously-scheduled job");
                }
            }
            work_queue.set(jobs_to_schedule);
        }

        let message = {
            let jobs_in_progress = scheduled_jobs
                .values()
                .filter_map(|scheduled_job| match scheduled_job {
                    ScheduledJob::Scheduled(job) => Some(job),
                    ScheduledJob::Complete(_) => None,
                })
                .collect_vec();
            if jobs_in_progress.is_empty() {
                debug!("No more in-progress jobs to wait on, exiting");
                break;
            }

            // If there is work to be done, then block on the next result to
            // be received from a worker. This is okay because we won't
            // adjust the work queue until we've received the next result.
            // (If we wanted to support cancellation or adjusting the number
            // of running workers then this wouldn't be sufficient.)
            debug!(?jobs_in_progress, "Event loop waiting for new job result");
            let result = result_rx.recv();
            debug!(?result, "Event loop got new job result");
            result
        };
        let (job, test_output) = match message {
            Err(RecvError) => {
                debug!("No more job results could be received because result_rx closed");
                break;
            }

            Ok(JobResult::Error(worker_id, job, error_message)) => {
                let TestJob {
                    commit_oid,
                    operation_type: _,
                } = job;
                eyre::bail!("Worker {worker_id} failed when processing commit {commit_oid}: {error_message}");
            }

            Ok(JobResult::Done(job, test_output)) => (job, test_output),
        };

        let TestJob {
            commit_oid,
            operation_type: _,
        } = job;
        let (maybe_testing_aborted_error, search_status) = match &test_output.test_status {
            TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::TerminatedBySignal
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_)
            | TestStatus::Indeterminate { .. } => (None, search::Status::Indeterminate),

            TestStatus::Abort { exit_code } => (
                Some(TestingAbortedError {
                    commit_oid,
                    exit_code: *exit_code,
                }),
                search::Status::Indeterminate,
            ),

            TestStatus::Failed {
                cached: _,
                interactive: _,
                exit_code: _,
            } => (None, search::Status::Failure),

            TestStatus::Passed {
                cached: _,
                fix_info: _,
                interactive: _,
            } => (None, search::Status::Success),
        };
        if search_strategy.is_some() {
            search.notify(commit_oid, search_status)?;
        }
        if scheduled_jobs
            .insert(commit_oid, ScheduledJob::Complete(test_output))
            .is_none()
        {
            warn!(
                ?commit_oid,
                "Received test result for commit that was not scheduled"
            );
        }

        if let Some(err) = maybe_testing_aborted_error {
            testing_aborted_error = Some(err);
        }
    }

    let test_outputs = scheduled_jobs
        .into_iter()
        .filter_map(|(commit_oid, scheduled_job)| match scheduled_job {
            ScheduledJob::Scheduled(_) => None,
            ScheduledJob::Complete(test_output) => Some((commit_oid, test_output)),
        })
        .collect();
    Ok(EventLoopOutput {
        search,
        test_outputs,
        testing_aborted_error,
    })
}

#[instrument]
fn print_summary(
    effects: &Effects,
    dag: &Dag,
    repo: &Repo,
    revset: &Revset,
    command: &TestCommand,
    test_results: &TestResults,
    is_search: bool,
    apply_fixes: bool,
    verbosity: &Verbosity,
) -> EyreExitOr<()> {
    let mut num_passed = 0;
    let mut num_failed = 0;
    let mut num_skipped = 0;
    let mut num_cached_results = 0;
    for (commit_oid, test_output) in &test_results.test_outputs {
        let commit = repo.find_commit_or_fail(*commit_oid)?;
        write!(
            effects.get_output_stream(),
            "{}",
            effects.get_glyphs().render(test_output.describe(
                effects,
                &commit,
                apply_fixes,
                *verbosity,
            )?)?
        )?;
        match test_output.test_status {
            TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_)
            | TestStatus::TerminatedBySignal
            | TestStatus::Indeterminate { .. } => num_skipped += 1,

            TestStatus::Abort { .. } => {
                num_failed += 1;
            }
            TestStatus::Failed {
                cached,
                exit_code: _,
                interactive: _,
            } => {
                num_failed += 1;
                if cached {
                    num_cached_results += 1;
                }
            }
            TestStatus::Passed {
                cached,
                fix_info: _,
                interactive: _,
            } => {
                num_passed += 1;
                if cached {
                    num_cached_results += 1;
                }
            }
        }
    }

    writeln!(
        effects.get_output_stream(),
        "Ran command on {}: {}",
        Pluralize {
            determiner: None,
            amount: test_results.test_outputs.len(),
            unit: ("commit", "commits")
        },
        effects.get_glyphs().render(
            StyledStringBuilder::new()
                .append_styled(command.to_string(), Effect::Bold)
                .build()
        )?,
    )?;

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

    if is_search {
        let success_commits: CommitSet =
            test_results.search_bounds.success.iter().copied().collect();
        let success_commits = sorted_commit_set(repo, dag, &success_commits)?;
        if success_commits.is_empty() {
            writeln!(
                effects.get_output_stream(),
                "There were no passing commits in the provided set."
            )?;
        } else {
            writeln!(
                effects.get_output_stream(),
                "Last passing {commits}:",
                commits = if success_commits.len() == 1 {
                    "commit"
                } else {
                    "commits"
                },
            )?;
            for commit in success_commits {
                writeln!(
                    effects.get_output_stream(),
                    "{} {}",
                    effects.get_glyphs().bullet_point,
                    effects
                        .get_glyphs()
                        .render(commit.friendly_describe(effects.get_glyphs())?)?
                )?;
            }
        }

        let failure_commits: CommitSet =
            test_results.search_bounds.failure.iter().copied().collect();
        let failure_commits = sorted_commit_set(repo, dag, &failure_commits)?;
        if failure_commits.is_empty() {
            writeln!(
                effects.get_output_stream(),
                "There were no failing commits in the provided set."
            )?;
        } else {
            writeln!(
                effects.get_output_stream(),
                "First failing {commits}:",
                commits = if failure_commits.len() == 1 {
                    "commit"
                } else {
                    "commits"
                },
            )?;
            for commit in failure_commits {
                writeln!(
                    effects.get_output_stream(),
                    "{} {}",
                    effects.get_glyphs().bullet_point,
                    effects
                        .get_glyphs()
                        .render(commit.friendly_describe(effects.get_glyphs())?)?
                )?;
            }
        }
    }

    if num_cached_results > 0 && get_hint_enabled(repo, Hint::CleanCachedTestResults)? {
        writeln!(
            effects.get_output_stream(),
            "{}: there {}",
            effects.get_glyphs().render(get_hint_string())?,
            Pluralize {
                determiner: Some(("was", "were")),
                amount: num_cached_results,
                unit: ("cached test result", "cached test results")
            }
        )?;
        writeln!(
            effects.get_output_stream(),
            "{}: to clear these cached results, run: git test clean {}",
            effects.get_glyphs().render(get_hint_string())?,
            shell_escape(revset.to_string()),
        )?;
        print_hint_suppression_notice(effects, Hint::CleanCachedTestResults)?;
    }

    if let Some(testing_aborted_error) = &test_results.testing_aborted_error {
        let TestingAbortedError {
            commit_oid,
            exit_code,
        } = testing_aborted_error;
        let commit = repo.find_commit_or_fail(*commit_oid)?;
        writeln!(
            effects.get_output_stream(),
            "Aborted running commands with exit code {} at commit: {}",
            exit_code,
            effects
                .get_glyphs()
                .render(commit.friendly_describe(effects.get_glyphs())?)?
        )?;
        return Ok(Err(ExitCode(1)));
    }

    if is_search {
        Ok(Ok(()))
    } else if num_failed > 0 || num_skipped > 0 {
        Ok(Err(ExitCode(1)))
    } else {
        Ok(Ok(()))
    }
}

#[instrument(skip(permissions))]
fn apply_fixes(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    dag: &mut Dag,
    repo: &Repo,
    event_log_db: &EventLogDb,
    execute_options: &ExecuteRebasePlanOptions,
    permissions: RebasePlanPermissions,
    dry_run: bool,
    command: &TestCommand,
    test_results: &TestResults,
) -> EyreExitOr<()> {
    let fixed_tree_oids: Vec<(NonZeroOid, NonZeroOid)> = test_results
        .test_outputs
        .iter()
        .filter_map(|(commit_oid, test_output)| match test_output.test_status {
            TestStatus::Passed {
                cached: _,
                fix_info:
                    FixInfo {
                        head_commit_oid: _,
                        snapshot_tree_oid: Some(snapshot_tree_oid),
                    },
                interactive: _,
            } => Some((*commit_oid, snapshot_tree_oid)),

            TestStatus::Passed {
                cached: _,
                fix_info:
                    FixInfo {
                        head_commit_oid: _,
                        snapshot_tree_oid: None,
                    },
                interactive: _,
            }
            | TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::TerminatedBySignal
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_)
            | TestStatus::Indeterminate { .. }
            | TestStatus::Failed { .. }
            | TestStatus::Abort { .. } => None,
        })
        .collect();

    #[derive(Debug)]
    struct Fix {
        original_commit_oid: NonZeroOid,
        original_commit_parent_oids: Vec<NonZeroOid>,
        fixed_commit_oid: NonZeroOid,
    }
    let fixes: Vec<Fix> = {
        let mut fixes = Vec::new();
        for (original_commit_oid, fixed_tree_oid) in fixed_tree_oids {
            let original_commit = repo.find_commit_or_fail(original_commit_oid)?;
            let original_tree_oid = original_commit.get_tree_oid();
            let commit_message = original_commit.get_message_raw();
            let commit_message = commit_message.to_str().with_context(|| {
                eyre::eyre!(
                    "Could not decode commit message for commit: {:?}",
                    original_commit_oid
                )
            })?;
            let parents: Vec<Commit> = original_commit
                .get_parent_oids()
                .into_iter()
                .map(|parent_oid| repo.find_commit_or_fail(parent_oid))
                .try_collect()?;
            let fixed_tree = repo.find_tree_or_fail(fixed_tree_oid)?;
            let fixed_commit_oid = repo.create_commit(
                None,
                &original_commit.get_author(),
                &original_commit.get_committer(),
                commit_message,
                &fixed_tree,
                parents.iter().collect(),
            )?;
            if original_commit_oid == fixed_commit_oid {
                continue;
            }

            let fix = Fix {
                original_commit_oid,
                original_commit_parent_oids: original_commit.get_parent_oids(),
                fixed_commit_oid,
            };
            debug!(
                ?fix,
                ?original_tree_oid,
                ?fixed_tree_oid,
                "Generated fix to apply"
            );
            fixes.push(fix);
        }
        fixes
    };

    dag.sync_from_oids(
        effects,
        repo,
        CommitSet::empty(),
        fixes
            .iter()
            .map(|fix| {
                let Fix {
                    original_commit_oid: _,
                    original_commit_parent_oids: _,
                    fixed_commit_oid,
                } = fix;
                fixed_commit_oid
            })
            .copied()
            .collect(),
    )?;

    let rebase_plan = {
        let mut builder = RebasePlanBuilder::new(dag, permissions);
        for fix in &fixes {
            let Fix {
                original_commit_oid,
                original_commit_parent_oids,
                fixed_commit_oid,
            } = fix;
            builder.replace_commit(*original_commit_oid, *fixed_commit_oid)?;
            builder.move_subtree(*original_commit_oid, original_commit_parent_oids.clone())?;
        }

        let original_oids: CommitSet = fixes
            .iter()
            .map(|fix| {
                let Fix {
                    original_commit_oid,
                    original_commit_parent_oids: _,
                    fixed_commit_oid: _,
                } = fix;
                original_commit_oid
            })
            .copied()
            .collect();
        let descendant_oids = dag.query_descendants(original_oids.clone())?;
        let descendant_oids = dag
            .filter_visible_commits(descendant_oids)?
            .difference(&original_oids);
        for descendant_oid in dag.commit_set_to_vec(&descendant_oids)? {
            let descendant_commit = repo.find_commit_or_fail(descendant_oid)?;
            builder.replace_commit(descendant_oid, descendant_oid)?;
            builder.move_subtree(descendant_oid, descendant_commit.get_parent_oids())?;
        }

        let thread_pool = ThreadPoolBuilder::new().build()?;
        let repo_pool = RepoResource::new_pool(repo)?;
        builder.build(effects, &thread_pool, &repo_pool)?
    };

    let rebase_plan = match rebase_plan {
        Ok(Some(plan)) => plan,
        Ok(None) => {
            writeln!(effects.get_output_stream(), "No commits to fix.")?;
            return Ok(Ok(()));
        }
        Err(err) => {
            err.describe(effects, repo, dag)?;
            return Ok(Err(ExitCode(1)));
        }
    };

    let rewritten_oids = if dry_run {
        Default::default()
    } else {
        match execute_rebase_plan(
            effects,
            git_run_info,
            repo,
            event_log_db,
            &rebase_plan,
            execute_options,
        )? {
            ExecuteRebasePlanResult::Succeeded { rewritten_oids } => rewritten_oids,
            ExecuteRebasePlanResult::DeclinedToMerge { failed_merge_info } => {
                writeln!(effects.get_output_stream(), "BUG: encountered merge conflicts during git test fix, but we should not be applying any patches: {failed_merge_info:?}")?;
                return Ok(Err(ExitCode(1)));
            }
            ExecuteRebasePlanResult::Failed { exit_code } => return Ok(Err(exit_code)),
        }
    };
    let rewritten_oids = match rewritten_oids {
        Some(rewritten_oids) => rewritten_oids,

        // Can happen during a dry-run; just produce our rewritten commits which
        // haven't been rebased on top of each other yet.
        // FIXME: it should be possible to execute the rebase plan but not
        // commit the branch moves so that we can preview it.
        None => fixes
            .iter()
            .map(|fix| {
                let Fix {
                    original_commit_oid,
                    original_commit_parent_oids: _,
                    fixed_commit_oid,
                } = fix;
                (
                    *original_commit_oid,
                    MaybeZeroOid::NonZero(*fixed_commit_oid),
                )
            })
            .collect(),
    };

    writeln!(
        effects.get_output_stream(),
        "Fixed {} with {}:",
        Pluralize {
            determiner: None,
            amount: fixes.len(),
            unit: ("commit", "commits")
        },
        effects.get_glyphs().render(
            StyledStringBuilder::new()
                .append_styled(command.to_string(), Effect::Bold)
                .build()
        )?,
    )?;
    for fix in fixes {
        let Fix {
            original_commit_oid,
            original_commit_parent_oids: _,
            fixed_commit_oid,
        } = fix;
        let original_commit = repo.find_commit_or_fail(original_commit_oid)?;
        let fixed_commit_oid = rewritten_oids
            .get(&original_commit_oid)
            .copied()
            .unwrap_or(MaybeZeroOid::NonZero(fixed_commit_oid));
        match fixed_commit_oid {
            MaybeZeroOid::NonZero(fixed_commit_oid) => {
                let fixed_commit = repo.find_commit_or_fail(fixed_commit_oid)?;
                writeln!(
                    effects.get_output_stream(),
                    "{} -> {}",
                    effects
                        .get_glyphs()
                        .render(original_commit.friendly_describe_oid(effects.get_glyphs())?)?,
                    effects
                        .get_glyphs()
                        .render(fixed_commit.friendly_describe(effects.get_glyphs())?)?
                )?;
            }

            MaybeZeroOid::Zero => {
                // Shouldn't happen.
                writeln!(
                    effects.get_output_stream(),
                    "(deleted) {}",
                    effects
                        .get_glyphs()
                        .render(original_commit.friendly_describe_oid(effects.get_glyphs())?)?,
                )?;
            }
        }
    }

    if dry_run {
        writeln!(effects.get_output_stream(), "(This was a dry-run, so no commits were rewritten. Re-run without the --dry-run option to apply fixes.)")?;
    }

    Ok(Ok(()))
}

#[instrument]
fn run_test(
    effects: &Effects,
    operation_type: OperationType,
    git_run_info: &GitRunInfo,
    shell_path: &Path,
    repo: &Repo,
    event_tx_id: EventTransactionId,
    options: &ResolvedTestOptions,
    worker_id: WorkerId,
    commit: &Commit,
) -> eyre::Result<TestOutput> {
    let ResolvedTestOptions {
        command: _, // Used in `test_commit`.
        execution_strategy,
        search_strategy: _, // Caller handles which commits to test.
        use_cache: _,       // Used only in `make_test_files`.
        is_dry_run: _,      // Used only in `apply_fixes`.
        is_interactive: _,  // Used in `test_commit`.
        num_jobs: _,        // Caller handles job management.
        verbosity: _,
        fix_options,
    } = options;
    let (effects, progress) = effects.start_operation(operation_type);
    progress.notify_status(
        OperationIcon::InProgress,
        format!(
            "Preparing {}",
            effects
                .get_glyphs()
                .render(commit.friendly_describe(effects.get_glyphs())?)?
        ),
    );

    let test_output = match make_test_files(repo, commit, options)? {
        TestFilesResult::Cached(test_output) => test_output,
        TestFilesResult::NotCached(test_files) => {
            match prepare_working_directory(
                git_run_info,
                repo,
                event_tx_id,
                commit,
                *execution_strategy,
                worker_id,
            )? {
                Err(err) => {
                    info!(?err, "Failed to prepare working directory for testing");
                    let TestFiles {
                        temp_dir,
                        lock_file: _, // Drop lock.
                        result_path,
                        result_file: _,
                        stdout_path,
                        stdout_file: _,
                        stderr_path,
                        stderr_file: _,
                    } = test_files;
                    TestOutput {
                        temp_dir,
                        result_path,
                        stdout_path,
                        stderr_path,
                        test_status: TestStatus::CheckoutFailed,
                    }
                }
                Ok(PreparedWorkingDirectory {
                    lock_file: mut working_directory_lock_file,
                    path,
                }) => {
                    progress.notify_status(
                        OperationIcon::InProgress,
                        format!(
                            "Running on {}",
                            effects
                                .get_glyphs()
                                .render(commit.friendly_describe(effects.get_glyphs())?)?
                        ),
                    );

                    let result = test_commit(
                        &effects,
                        git_run_info,
                        repo,
                        event_tx_id,
                        test_files,
                        &path,
                        shell_path,
                        options,
                        commit,
                    )?;
                    working_directory_lock_file
                        .unlock()
                        .wrap_err_with(|| format!("Unlocking working directory at {path:?}"))?;
                    drop(working_directory_lock_file);
                    result
                }
            }
        }
    };

    let description = StyledStringBuilder::new()
        .append(test_output.test_status.describe(
            effects.get_glyphs(),
            commit,
            fix_options.is_some(),
        )?)
        .build();
    progress.notify_status(
        match test_output.test_status {
            TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_)
            | TestStatus::Indeterminate { .. } => OperationIcon::Warning,

            TestStatus::TerminatedBySignal
            | TestStatus::Failed { .. }
            | TestStatus::Abort { .. } => OperationIcon::Failure,

            TestStatus::Passed { .. } => OperationIcon::Success,
        },
        effects.get_glyphs().render(description)?,
    );
    Ok(test_output)
}

#[derive(Debug)]
struct TestFiles {
    temp_dir: Option<TempDir>,
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
    if !options.use_cache {
        let temp_dir = tempfile::tempdir().context("Creating temporary directory")?;
        let lock_path = temp_dir.path().join("pid.lock");
        let mut lock_file = LockFile::open(&lock_path)
            .wrap_err_with(|| format!("Opening lock file {lock_path:?}"))?;
        if !lock_file.try_lock_with_pid()? {
            warn!(
                ?temp_dir,
                ?lock_file,
                "Could not acquire lock despite being in a temporary directory"
            );
        }

        let result_path = temp_dir.path().join("result");
        let stdout_path = temp_dir.path().join("stdout");
        let stderr_path = temp_dir.path().join("stderr");
        let result_file = File::create(&result_path)
            .wrap_err_with(|| format!("Opening result file {result_path:?}"))?;
        let stdout_file = File::create(&stdout_path)
            .wrap_err_with(|| format!("Opening stdout file {stdout_path:?}"))?;
        let stderr_file = File::create(&stderr_path)
            .wrap_err_with(|| format!("Opening stderr file {stderr_path:?}"))?;
        return Ok(TestFilesResult::NotCached(TestFiles {
            temp_dir: Some(temp_dir),
            lock_file,
            result_path,
            result_file,
            stdout_path,
            stdout_file,
            stderr_path,
            stderr_file,
        }));
    }

    let tree_dir = get_test_tree_dir(repo, commit)?;
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
            temp_dir: None,
            result_path,
            stdout_path,
            stderr_path,
            test_status: TestStatus::AlreadyInProgress,
        }));
    }

    if let Ok(contents) = std::fs::read_to_string(&result_path) {
        // If the file exists but was empty, this indicates that a previous
        // attempt did not complete successfully. However, we successfully took
        // the lock, so it should be the case that we are the exclusive writers
        // to the contents of this directory (i.e. the previous attempt is not
        // still running), so it's safe to proceed and overwrite these files.
        if !contents.is_empty() {
            let serialized_result: Result<SerializedTestResult, _> =
                serde_json::from_str(&contents);
            let test_status = match serialized_result {
                Ok(SerializedTestResult {
                    command: _,
                    exit_code: 0,
                    head_commit_oid,
                    snapshot_tree_oid,
                    interactive,
                }) => TestStatus::Passed {
                    cached: true,
                    fix_info: FixInfo {
                        head_commit_oid: head_commit_oid.map(|SerializedNonZeroOid(oid)| oid),
                        snapshot_tree_oid: snapshot_tree_oid.map(|SerializedNonZeroOid(oid)| oid),
                    },

                    interactive,
                },

                Ok(SerializedTestResult {
                    command: _,
                    exit_code,
                    head_commit_oid: _,
                    snapshot_tree_oid: _,
                    interactive: _,
                }) if exit_code == TEST_INDETERMINATE_EXIT_CODE => {
                    TestStatus::Indeterminate { exit_code }
                }

                Ok(SerializedTestResult {
                    command: _,
                    exit_code,
                    head_commit_oid: _,
                    snapshot_tree_oid: _,
                    interactive: _,
                }) if exit_code == TEST_ABORT_EXIT_CODE => TestStatus::Abort { exit_code },

                Ok(SerializedTestResult {
                    command: _,
                    exit_code,
                    head_commit_oid: _,
                    snapshot_tree_oid: _,
                    interactive,
                }) => TestStatus::Failed {
                    cached: true,
                    exit_code,
                    interactive,
                },
                Err(err) => TestStatus::ReadCacheFailed(err.to_string()),
            };
            return Ok(TestFilesResult::Cached(TestOutput {
                temp_dir: None,
                result_path,
                stdout_path,
                stderr_path,
                test_status,
            }));
        }
    }

    let result_file = File::create(&result_path)
        .wrap_err_with(|| format!("Opening result file {result_path:?}"))?;
    let stdout_file = File::create(&stdout_path)
        .wrap_err_with(|| format!("Opening stdout file {stdout_path:?}"))?;
    let stderr_file = File::create(&stderr_path)
        .wrap_err_with(|| format!("Opening stderr file {stderr_path:?}"))?;
    Ok(TestFilesResult::NotCached(TestFiles {
        temp_dir: None,
        lock_file,
        result_path,
        result_file,
        stdout_path,
        stdout_file,
        stderr_path,
        stderr_file,
    }))
}

#[derive(Debug)]
struct PreparedWorkingDirectory {
    lock_file: LockFile,
    path: PathBuf,
}

#[allow(dead_code)] // fields are not read except by `Debug` implementation`
#[derive(Debug)]
enum PrepareWorkingDirectoryError {
    LockFailed(PathBuf),
    NoWorkingCopy,
    CheckoutFailed(NonZeroOid),
    CreateWorktreeFailed(PathBuf),
}

#[instrument]
fn prepare_working_directory(
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_tx_id: EventTransactionId,
    commit: &Commit,
    strategy: TestExecutionStrategy,
    worker_id: WorkerId,
) -> eyre::Result<Result<PreparedWorkingDirectory, PrepareWorkingDirectoryError>> {
    let test_lock_dir_path = get_test_locks_dir(repo)?;
    std::fs::create_dir_all(&test_lock_dir_path)
        .wrap_err_with(|| format!("Creating test lock dir path: {test_lock_dir_path:?}"))?;

    let lock_file_name = match strategy {
        TestExecutionStrategy::WorkingCopy => "working-copy.lock".to_string(),
        TestExecutionStrategy::Worktree => {
            format!("worktree-{worker_id}.lock")
        }
    };
    let lock_path = test_lock_dir_path.join(lock_file_name);
    let mut lock_file = LockFile::open(&lock_path)
        .wrap_err_with(|| format!("Opening working copy lock at {lock_path:?}"))?;
    if !lock_file
        .try_lock_with_pid()
        .wrap_err_with(|| format!("Locking working copy with {lock_path:?}"))?
    {
        return Ok(Err(PrepareWorkingDirectoryError::LockFailed(lock_path)));
    }

    match strategy {
        TestExecutionStrategy::WorkingCopy => {
            let working_copy_path = match repo.get_working_copy_path() {
                None => return Ok(Err(PrepareWorkingDirectoryError::NoWorkingCopy)),
                Some(working_copy_path) => working_copy_path.to_owned(),
            };

            let GitRunResult { exit_code, stdout: _, stderr: _ } =
                // Don't show the `git reset` operation among the progress bars,
                // as we only want to see the testing status.
                git_run_info.run_silent(
                    repo,
                    Some(event_tx_id),
                    &["reset", "--hard", &commit.get_oid().to_string()],
                    Default::default()
                ).context("Checking out commit to prepare working directory")?;
            if exit_code.is_success() {
                Ok(Ok(PreparedWorkingDirectory {
                    lock_file,
                    path: working_copy_path,
                }))
            } else {
                Ok(Err(PrepareWorkingDirectoryError::CheckoutFailed(
                    commit.get_oid(),
                )))
            }
        }

        TestExecutionStrategy::Worktree => {
            let parent_dir = get_test_worktrees_dir(repo)?;
            std::fs::create_dir_all(&parent_dir)
                .wrap_err_with(|| format!("Creating worktree parent dir at {parent_dir:?}"))?;

            let worktree_dir_name = format!("testing-worktree-{worker_id}");
            let worktree_dir = parent_dir.join(worktree_dir_name);
            let worktree_dir_str = match worktree_dir.to_str() {
                Some(worktree_dir) => worktree_dir,
                None => {
                    return Ok(Err(PrepareWorkingDirectoryError::CreateWorktreeFailed(
                        worktree_dir,
                    )));
                }
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
                    return Ok(Err(PrepareWorkingDirectoryError::CreateWorktreeFailed(
                        worktree_dir,
                    )));
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
                return Ok(Err(PrepareWorkingDirectoryError::CheckoutFailed(
                    commit.get_oid(),
                )));
            }
            Ok(Ok(PreparedWorkingDirectory {
                lock_file,
                path: worktree_dir,
            }))
        }
    }
}

#[instrument]
fn test_commit(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_tx_id: EventTransactionId,
    test_files: TestFiles,
    working_directory: &Path,
    shell_path: &Path,
    options: &ResolvedTestOptions,
    commit: &Commit,
) -> eyre::Result<TestOutput> {
    let TestFiles {
        temp_dir,
        lock_file: _lock_file, // Make sure not to drop lock.
        result_path,
        result_file,
        stdout_path,
        stdout_file,
        stderr_path,
        stderr_file,
    } = test_files;

    let mut command = Command::new(shell_path);
    command
        .arg("-c")
        .arg(options.command.to_string())
        .current_dir(working_directory)
        .env(BRANCHLESS_TRANSACTION_ID_ENV_VAR, event_tx_id.to_string())
        .env("BRANCHLESS_TEST_COMMIT", commit.get_oid().to_string())
        .env("BRANCHLESS_TEST_COMMAND", options.command.to_string());

    if options.is_interactive {
        let commit_desc = effects
            .get_glyphs()
            .render(commit.friendly_describe(effects.get_glyphs())?)?;
        let passed = "passed";
        let exit0 = effects
            .get_glyphs()
            .render(StyledString::styled("exit 0", *STYLE_SUCCESS))?;
        let failed = "failed";
        let exit1 = effects
            .get_glyphs()
            .render(StyledString::styled("exit 1", *STYLE_FAILURE))?;
        let skipped = "skipped";
        let exit125 = effects
            .get_glyphs()
            .render(StyledString::styled("exit 125", *STYLE_SKIPPED))?;
        let exit127 = effects
            .get_glyphs()
            .render(StyledString::styled("exit 127", *STYLE_FAILURE))?;

        // NB: use `println` here instead of
        // `writeln!(effects.get_output_stream(), ...)` because the effects are
        // suppressed in interactive mode.
        println!(
            "\
You are now at: {commit_desc}
To mark this commit as {passed},run:   {exit0}
To mark this commit as {failed}, run:  {exit1}
To mark this commit as {skipped}, run: {exit125}
To abort testing entirely, run:      {exit127}",
        );
        match options.execution_strategy {
            TestExecutionStrategy::WorkingCopy => {}
            TestExecutionStrategy::Worktree => {
                let warning = effects
                    .get_glyphs()
                    .render(StyledString::styled(
                        "Warning: You are in a worktree. Your changes will not be propagated between the worktree and the main repository.",
                        *STYLE_SKIPPED
                    ))?;
                println!("{warning}");
                println!("To save your changes, create a new branch or note the commit hash.");
                println!("To incorporate the changes from the main repository, switch to the main repository's current commit or branch.");
            }
        }
    } else {
        command
            .stdin(Stdio::null())
            .stdout(stdout_file)
            .stderr(stderr_file);
    }

    let exit_code = match command.status() {
        Ok(status) => status.code(),
        Err(err) => {
            return Ok(TestOutput {
                temp_dir,
                result_path,
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
                temp_dir,
                result_path,
                stdout_path,
                stderr_path,
                test_status: TestStatus::TerminatedBySignal,
            });
        }
    };
    let test_status = match exit_code {
        TEST_SUCCESS_EXIT_CODE => {
            let fix_info = {
                let repo = Repo::from_dir(working_directory)?;
                let (head_commit_oid, snapshot) = {
                    let index = repo.get_index()?;
                    let head_info = repo.get_head_info()?;
                    let (snapshot, _status) = repo.get_status(
                        &effects.suppress(),
                        git_run_info,
                        &index,
                        &head_info,
                        Some(event_tx_id),
                    )?;
                    (head_info.oid, snapshot)
                };
                let snapshot_tree_oid = match snapshot.get_working_copy_changes_type()? {
                    WorkingCopyChangesType::None | WorkingCopyChangesType::Unstaged => {
                        let fixed_tree_oid: MaybeZeroOid = snapshot.commit_unstaged.get_tree_oid();
                        fixed_tree_oid.into()
                    }
                    changes_type @ (WorkingCopyChangesType::Staged
                    | WorkingCopyChangesType::Conflicts) => {
                        // FIXME: surface information about the fix that failed to be applied.
                        warn!(
                            ?changes_type,
                            "There were staged changes or conflicts in the resulting working copy"
                        );
                        None
                    }
                };
                FixInfo {
                    head_commit_oid,
                    snapshot_tree_oid,
                }
            };
            TestStatus::Passed {
                cached: false,
                fix_info,
                interactive: options.is_interactive,
            }
        }

        exit_code @ TEST_INDETERMINATE_EXIT_CODE => TestStatus::Indeterminate { exit_code },
        exit_code @ TEST_ABORT_EXIT_CODE => TestStatus::Abort { exit_code },

        exit_code => TestStatus::Failed {
            cached: false,
            exit_code,
            interactive: options.is_interactive,
        },
    };

    let fix_info = match &test_status {
        TestStatus::Passed {
            cached: _,
            fix_info,
            interactive: _,
        } => Some(fix_info),
        TestStatus::CheckoutFailed
        | TestStatus::SpawnTestFailed(_)
        | TestStatus::TerminatedBySignal
        | TestStatus::AlreadyInProgress
        | TestStatus::ReadCacheFailed(_)
        | TestStatus::Failed { .. }
        | TestStatus::Abort { .. }
        | TestStatus::Indeterminate { .. } => None,
    };
    let serialized_test_result = SerializedTestResult {
        command: options.command.clone(),
        exit_code,
        head_commit_oid: fix_info
            .and_then(|fix_info| fix_info.head_commit_oid.map(SerializedNonZeroOid)),
        snapshot_tree_oid: fix_info
            .and_then(|fix_info| fix_info.snapshot_tree_oid.map(SerializedNonZeroOid)),
        interactive: options.is_interactive,
    };
    serde_json::to_writer_pretty(result_file, &serialized_test_result)
        .wrap_err_with(|| format!("Writing test status {test_status:?} to {result_path:?}"))?;

    Ok(TestOutput {
        temp_dir,
        result_path,
        stdout_path,
        stderr_path,
        test_status,
    })
}

/// Show test output for the command provided in `options` for each of the
/// commits in `revset`.
#[instrument]
fn subcommand_show(
    effects: &Effects,
    options: &RawTestOptions,
    revset: Revset,
    resolve_revset_options: &ResolveRevsetOptions,
) -> EyreExitOr<()> {
    let now = SystemTime::now();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "test show")?;
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

    let commit_set =
        match resolve_commits(effects, &repo, &mut dag, &[revset], resolve_revset_options) {
            Ok(mut commit_sets) => commit_sets.pop().unwrap(),
            Err(err) => {
                err.describe(effects)?;
                return Ok(Err(ExitCode(1)));
            }
        };

    let options = try_exit_code!(ResolvedTestOptions::resolve(
        now,
        effects,
        &dag,
        &repo,
        event_tx_id,
        &commit_set,
        None,
        options,
    )?);

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
                        false,
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
                    "{}: to see more detailed output, re-run with -v/--verbose",
                    effects.get_glyphs().render(get_hint_string())?,
                )?;
                print_hint_suppression_notice(effects, Hint::TestShowVerbose)?;
            }
            Verbosity::PartialOutput => {
                writeln!(
                    effects.get_output_stream(),
                    "{}: to see more detailed output, re-run with -vv/--verbose --verbose",
                    effects.get_glyphs().render(get_hint_string())?,
                )?;
                print_hint_suppression_notice(effects, Hint::TestShowVerbose)?;
            }
            Verbosity::FullOutput => {}
        }
    }

    Ok(Ok(()))
}

/// Delete cached test output for the commits in `revset`.
#[instrument]
pub fn subcommand_clean(
    effects: &Effects,
    revset: Revset,
    resolve_revset_options: &ResolveRevsetOptions,
) -> EyreExitOr<()> {
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

    let commit_set =
        match resolve_commits(effects, &repo, &mut dag, &[revset], resolve_revset_options) {
            Ok(mut commit_sets) => commit_sets.pop().unwrap(),
            Err(err) => {
                err.describe(effects)?;
                return Ok(Err(ExitCode(1)));
            }
        };

    let mut num_cleaned_commits = 0;
    for commit in sorted_commit_set(&repo, &dag, &commit_set)? {
        let tree_dir = get_test_tree_dir(&repo, &commit)?;
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
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use lib::testing::make_git;

    use super::*;

    #[test]
    fn test_lock_prepared_working_directory() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        let git_run_info = git.get_git_run_info();
        let repo = git.get_repo()?;
        let conn = repo.get_db_conn()?;
        let event_log_db = EventLogDb::new(&conn)?;
        let event_tx_id = event_log_db.make_transaction_id(SystemTime::now(), "test")?;
        let head_oid = repo.get_head_info()?.oid.unwrap();
        let head_commit = repo.find_commit_or_fail(head_oid)?;
        let worker_id = 1;

        let _prepared_working_copy = prepare_working_directory(
            &git_run_info,
            &repo,
            event_tx_id,
            &head_commit,
            TestExecutionStrategy::WorkingCopy,
            worker_id,
        )?
        .unwrap();
        assert!(matches!(
            prepare_working_directory(
                &git_run_info,
                &repo,
                event_tx_id,
                &head_commit,
                TestExecutionStrategy::WorkingCopy,
                worker_id
            )?,
            Err(PrepareWorkingDirectoryError::LockFailed(_))
        ));

        let _prepared_worktree = prepare_working_directory(
            &git_run_info,
            &repo,
            event_tx_id,
            &head_commit,
            TestExecutionStrategy::Worktree,
            worker_id,
        )?
        .unwrap();
        assert!(matches!(
            prepare_working_directory(
                &git_run_info,
                &repo,
                event_tx_id,
                &head_commit,
                TestExecutionStrategy::Worktree,
                worker_id
            )?,
            Err(PrepareWorkingDirectoryError::LockFailed(_))
        ));

        Ok(())
    }
}
