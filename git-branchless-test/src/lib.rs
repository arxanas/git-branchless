//! Run a user-provided command on each of a set of provided commits. This is
//! useful to run checks on all commits in the current stack with caching,
//! parallelization, out-of-tree execution, etc.

#![warn(missing_docs)]
#![warn(clippy::all, clippy::as_conversions, clippy::clone_on_ref_ptr)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

use std::collections::{HashMap, VecDeque};
use std::convert::TryFrom;
use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use bstr::ByteSlice;
use clap::ValueEnum;
use cursive::theme::{BaseColor, Effect, Style};
use cursive::utils::markup::StyledString;
use eyre::WrapErr;
use fslock::LockFile;
use git_branchless_invoke::CommandContext;
use itertools::Itertools;
use lazy_static::lazy_static;
use lib::core::check_out::CheckOutCommitOptions;
use lib::core::config::{
    get_hint_enabled, get_hint_string, get_restack_preserve_timestamps,
    print_hint_suppression_notice, Hint,
};
use lib::core::dag::{sorted_commit_set, CommitSet, Dag};
use lib::core::effects::{icons, Effects, OperationIcon, OperationType, ProgressHandle};
use lib::core::eventlog::{EventLogDb, EventReplayer, EventTransactionId};
use lib::core::formatting::{Glyphs, Pluralize, StyledStringBuilder};
use lib::core::repo_ext::RepoExt;
use lib::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanOptions, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    RebaseCommand, RebasePlan, RebasePlanBuilder, RebasePlanPermissions, RepoResource,
};
use lib::git::{
    Commit, ConfigRead, GitRunInfo, GitRunResult, MaybeZeroOid, NonZeroOid, Repo,
    WorkingCopyChangesType,
};
use lib::util::{get_sh, ExitCode};
use rayon::ThreadPoolBuilder;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use tracing::{debug, instrument, warn};

use git_branchless_opts::{
    MoveOptions, ResolveRevsetOptions, Revset, TestArgs, TestExecutionStrategy, TestSubcommand,
};
use git_branchless_revset::resolve_commits;

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
enum Verbosity {
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
struct RawTestOptions {
    /// The command to execute, if any.
    pub exec: Option<String>,

    /// The command alias to execute, if any.
    pub command: Option<String>,

    /// The execution strategy to use.
    pub strategy: Option<TestExecutionStrategy>,

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
    jobs: usize,
    verbosity: Verbosity,
    fix_options: Option<(ExecuteRebasePlanOptions, RebasePlanPermissions)>,
}

impl ResolvedTestOptions {
    fn resolve(
        now: SystemTime,
        effects: &Effects,
        dag: &Dag,
        repo: &Repo,
        event_tx_id: EventTransactionId,
        commits: &CommitSet,
        move_options: Option<&MoveOptions>,
        options: &RawTestOptions,
    ) -> eyre::Result<Result<Self, ExitCode>> {
        let config = repo.get_readonly_config()?;
        let RawTestOptions {
            exec: command,
            command: command_alias,
            strategy,
            jobs,
            verbosity,
            apply_fixes,
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
        let (resolved_jobs, resolved_strategy) = match jobs {
            None => match configured_jobs {
                None => (1, resolved_strategy),
                Some(jobs) => match usize::try_from(jobs) {
                    Ok(jobs) => (jobs, resolved_strategy),
                    Err(err) => {
                        writeln!(
                            effects.get_output_stream(),
                            "Invalid value for config value for {jobs_config_key}: {err}"
                        )?;
                        return Ok(Err(ExitCode(1)));
                    }
                },
            },
            Some(1) => (1, resolved_strategy),
            Some(jobs) => {
                // NB: match on the strategy passed on the command-line here, not the resolved strategy.
                match strategy {
                    None | Some(TestExecutionStrategy::Worktree) => {
                        (*jobs, TestExecutionStrategy::Worktree)
                    }
                    Some(TestExecutionStrategy::WorkingCopy) => {
                        writeln!(
                            effects.get_output_stream(),
                            "\
The --jobs argument can only be used with --strategy worktree,
but --strategy working-copy was provided instead."
                        )?;
                        return Ok(Err(ExitCode(1)));
                    }
                }
            }
        };

        let resolved_jobs = if resolved_jobs == 0 {
            num_cpus::get_physical()
        } else {
            resolved_jobs
        };
        assert!(resolved_jobs > 0);

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
                force_in_memory,
                force_on_disk,
                detect_duplicate_commits_via_patch_id,
                resolve_merge_conflicts,
                dump_rebase_constraints,
                dump_rebase_plan,
            } = move_options;
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
                force_in_memory: *force_in_memory,
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
                        err.describe(effects, repo)?;
                        return Ok(Err(ExitCode(1)));
                    }
                };
            Some((execute_options, permissions))
        } else {
            None
        };

        Ok(Ok(ResolvedTestOptions {
            command: resolved_command,
            strategy: resolved_strategy,
            jobs: resolved_jobs,
            verbosity: *verbosity,
            fix_options,
        }))
    }

    fn make_command_slug(&self) -> String {
        self.command.replace(['/', ' ', '\n'], "__")
    }
}

/// `test` command.
#[instrument]
pub fn command_main(ctx: CommandContext, args: TestArgs) -> eyre::Result<ExitCode> {
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
            jobs,
        } => subcommand_run(
            &effects,
            &git_run_info,
            &RawTestOptions {
                exec: command,
                command: command_alias,
                strategy,
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
                strategy: None,
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
            revset,
            resolve_revset_options,
            verbosity,
            strategy,
            jobs,
            move_options,
        } => subcommand_run(
            &effects,
            &git_run_info,
            &RawTestOptions {
                exec: command,
                command: command_alias,
                strategy,
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
) -> eyre::Result<ExitCode> {
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
        &[revset.clone()],
        resolve_revset_options,
    ) {
        Ok(mut commit_sets) => commit_sets.pop().unwrap(),
        Err(err) => {
            err.describe(effects)?;
            return Ok(ExitCode(1));
        }
    };

    let options = match ResolvedTestOptions::resolve(
        now,
        effects,
        &dag,
        &repo,
        event_tx_id,
        &commit_set,
        move_options,
        options,
    )? {
        Ok(options) => options,
        Err(exit_code) => return Ok(exit_code),
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
    let results: Result<_, _> = run_tests(
        effects,
        git_run_info,
        &dag,
        &repo,
        &event_log_db,
        event_tx_id,
        &revset,
        &commits,
        &options,
    );
    let abort_trap_exit_code = clear_abort_trap(effects, git_run_info, event_tx_id, abort_trap)?;
    if !abort_trap_exit_code.is_success() {
        return Ok(abort_trap_exit_code);
    }

    let results = match results? {
        Ok(results) => results,
        Err(exit_code) => return Ok(exit_code),
    };

    let exit_code = print_summary(
        effects,
        &repo,
        &revset,
        &options.command,
        &results,
        &options.verbosity,
    )?;
    if !exit_code.is_success() {
        return Ok(exit_code);
    }

    if let Some((execute_options, permissions)) = &options.fix_options {
        let exit_code = apply_fixes(
            effects,
            git_run_info,
            &dag,
            &repo,
            &event_log_db,
            execute_options,
            permissions.clone(),
            &options.command,
            &results,
        )?;
        if !exit_code.is_success() {
            return Ok(exit_code);
        }
    }

    Ok(ExitCode(0))
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
            check_out_commit_options: Default::default(),
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
#[derive(Debug)]
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
    Failed {
        /// Whether or not the result was cached (indicating that we didn't
        /// actually re-run the test).
        cached: bool,

        /// The exit code of the process.
        exit_code: i32,
    },

    /// The test passed and returned a successful exit code.
    Passed {
        /// Whether or not the result was cached (indicating that we didn't
        /// actually re-run the test).
        cached: bool,
        fixed_tree_oid: Option<NonZeroOid>,
    },
}

impl TestStatus {
    #[instrument]
    fn get_icon(&self) -> &'static str {
        match self {
            TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_)
            | TestStatus::TerminatedBySignal => icons::EXCLAMATION,
            TestStatus::Failed { .. } => icons::CROSS,
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
            | TestStatus::TerminatedBySignal => *STYLE_SKIPPED,
            TestStatus::Failed { .. } => *STYLE_FAILURE,
            TestStatus::Passed { .. } => *STYLE_SUCCESS,
        }
    }
}

#[derive(Debug)]
struct SerializedNonZeroOid(NonZeroOid);

impl Serialize for SerializedNonZeroOid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for SerializedNonZeroOid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let oid: NonZeroOid = s.parse().map_err(|_| {
            de::Error::invalid_value(de::Unexpected::Str(&s), &"a valid non-zero OID")
        })?;
        Ok(SerializedNonZeroOid(oid))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SerializedTestResult {
    command: String,
    exit_code: i32,
    fixed_tree_oid: Option<SerializedNonZeroOid>,
}

#[instrument]
fn make_test_status_description(
    glyphs: &Glyphs,
    commit: &Commit,
    test_status: &TestStatus,
) -> eyre::Result<StyledString> {
    let description = match test_status {
        TestStatus::CheckoutFailed => StyledStringBuilder::new()
            .append_styled("Failed to check out: ", *STYLE_SKIPPED)
            .append(commit.friendly_describe(glyphs)?)
            .build(),

        TestStatus::SpawnTestFailed(err) => StyledStringBuilder::new()
            .append_styled(format!("Failed to spawn test: {err}: "), *STYLE_SKIPPED)
            .append(commit.friendly_describe(glyphs)?)
            .build(),

        TestStatus::TerminatedBySignal => StyledStringBuilder::new()
            .append_styled("Test command terminated by signal: ", *STYLE_FAILURE)
            .append(commit.friendly_describe(glyphs)?)
            .build(),

        TestStatus::AlreadyInProgress => StyledStringBuilder::new()
            .append_styled("Test already in progress? ", *STYLE_SKIPPED)
            .append(commit.friendly_describe(glyphs)?)
            .build(),

        TestStatus::ReadCacheFailed(_) => StyledStringBuilder::new()
            .append_styled("Could not read cached test result: ", *STYLE_SKIPPED)
            .append(commit.friendly_describe(glyphs)?)
            .build(),

        TestStatus::Failed {
            cached: false,
            exit_code,
        } => StyledStringBuilder::new()
            .append_styled(format!("Failed (exit code {exit_code}): "), *STYLE_FAILURE)
            .append(commit.friendly_describe(glyphs)?)
            .build(),

        TestStatus::Failed {
            cached: true,
            exit_code,
        } => StyledStringBuilder::new()
            .append_styled(
                format!("Failed (cached, exit code {exit_code}): "),
                *STYLE_FAILURE,
            )
            .append(commit.friendly_describe(glyphs)?)
            .build(),

        TestStatus::Passed {
            cached: false,
            fixed_tree_oid: None,
        } => StyledStringBuilder::new()
            .append_styled("Passed: ", *STYLE_SUCCESS)
            .append(commit.friendly_describe(glyphs)?)
            .build(),

        TestStatus::Passed {
            cached: false,
            fixed_tree_oid: Some(_),
        } => StyledStringBuilder::new()
            .append_styled("Passed (fixed): ", *STYLE_SUCCESS)
            .append(commit.friendly_describe(glyphs)?)
            .build(),

        TestStatus::Passed {
            cached: true,
            fixed_tree_oid: None,
        } => StyledStringBuilder::new()
            .append_styled("Passed (cached): ", *STYLE_SUCCESS)
            .append(commit.friendly_describe(glyphs)?)
            .build(),

        TestStatus::Passed {
            cached: true,
            fixed_tree_oid: Some(_),
        } => StyledStringBuilder::new()
            .append_styled("Passed (cached, fixed): ", *STYLE_SUCCESS)
            .append(commit.friendly_describe(glyphs)?)
            .build(),
    };
    Ok(description)
}

impl TestOutput {
    #[instrument]
    fn describe(
        &self,
        effects: &Effects,
        commit: &Commit,
        verbosity: Verbosity,
    ) -> eyre::Result<StyledString> {
        let description = StyledStringBuilder::new()
            .append_styled(self.test_status.get_icon(), self.test_status.get_style())
            .append_plain(" ")
            .append(make_test_status_description(
                effects.get_glyphs(),
                commit,
                &self.test_status,
            )?)
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

fn shell_escape(s: impl AsRef<str>) -> String {
    let s = s.as_ref();
    let mut escaped = String::new();
    escaped.push('"');
    for c in s.chars() {
        match c {
            '"' => escaped.push_str(r#"\""#),
            '\\' => escaped.push_str(r#"\\\\"#),
            c => escaped.push(c),
        }
    }
    escaped.push('"');
    escaped
}

#[instrument]
fn run_tests(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    dag: &Dag,
    repo: &Repo,
    event_log_db: &EventLogDb,
    event_tx_id: EventTransactionId,
    revset: &Revset,
    commits: &[Commit],
    options: &ResolvedTestOptions,
) -> eyre::Result<Result<Vec<(NonZeroOid, TestOutput)>, ExitCode>> {
    let ResolvedTestOptions {
        command,
        strategy,
        jobs,
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

    if let Some(strategy_value) = strategy.to_possible_value() {
        writeln!(
            effects.get_output_stream(),
            "Using test execution strategy: {}",
            effects.get_glyphs().render(
                StyledStringBuilder::new()
                    .append_styled(strategy_value.get_name(), Effect::Bold)
                    .build()
            )?,
        )?;
    }

    let results = {
        let (effects, progress) =
            effects.start_operation(OperationType::RunTests(Arc::new(command.clone())));
        progress.notify_progress(0, commits.len());
        let work_queue = {
            let mut results = VecDeque::new();
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
                    format!("Waiting to test {commit_description}"),
                );
                results.push_back((commit.get_oid(), operation_type));
            }
            results
        };

        let repo_dir = repo.get_path();
        crossbeam::thread::scope(|scope| {
            let work_queue = Arc::new(Mutex::new(work_queue));
            let (result_tx, result_rx) = channel();
            let workers: HashMap<WorkerId, crossbeam::thread::ScopedJoinHandle<()>> = {
                let mut result = HashMap::new();
                for worker_id in 1..=*jobs {
                    let effects = &effects;
                    let progress = &progress;
                    let shell_path = &shell_path;
                    let work_queue = Arc::clone(&work_queue);
                    let result_tx = result_tx.clone();
                    result.insert(
                        worker_id,
                        scope.spawn(move |_scope| {
                            worker(
                                effects,
                                progress,
                                shell_path,
                                git_run_info,
                                repo_dir,
                                event_tx_id,
                                options,
                                worker_id,
                                work_queue,
                                result_tx,
                            );
                            debug!("Exiting spawned thread closure");
                        }),
                    );
                }
                result
            };

            // We rely on `result_rx.recv()` returning `Err` once all the threads have exited
            // (whether that reason should be panicking or having finished all work). We have to
            // drop our local reference to ensure that we don't deadlock, since otherwise therex
            // would still be a live receiver for the sender.
            drop(result_tx);

            let mut results = Vec::new();
            while let Ok(message) = {
                debug!("Main thread waiting for new job result");
                let result = result_rx.recv();
                debug!("Main thread got new job result");
                result
            } {
                match message {
                    JobResult::Done(commit_oid, test_output) => {
                        results.push((commit_oid, test_output));
                        if results.len() == commits.len() {
                            drop(result_rx);
                            break;
                        }
                    }
                    JobResult::Error(worker_id, commit_oid, error_message) => {
                        eyre::bail!("Worker {worker_id} failed when processing commit {commit_oid}: {error_message}");
                    }
                }
            }

            debug!("Waiting for workers");
            progress.notify_status(OperationIcon::InProgress, "Waiting for workers");
            for (worker_id, worker) in workers {
                worker
                    .join()
                    .map_err(|_err| eyre::eyre!("Waiting for worker {worker_id} to exit"))?;
            }

            debug!("About to return from thread scope");
            Ok(results)
        }).map_err(|_| eyre::eyre!("Could not spawn workers"))?.wrap_err("Failed waiting on workers")?
    };
    debug!("Returned from thread scope");

    Ok(Ok(results))
}

fn print_summary(
    effects: &Effects,
    repo: &Repo,
    revset: &Revset,
    command: &str,
    results: &[(NonZeroOid, TestOutput)],
    verbosity: &Verbosity,
) -> eyre::Result<ExitCode> {
    let mut num_passed = 0;
    let mut num_failed = 0;
    let mut num_skipped = 0;
    let mut num_cached_results = 0;
    for (commit_oid, test_output) in results {
        let commit = repo.find_commit_or_fail(*commit_oid)?;
        write!(
            effects.get_output_stream(),
            "{}",
            effects
                .get_glyphs()
                .render(test_output.describe(effects, &commit, *verbosity)?)?
        )?;
        match test_output.test_status {
            TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_)
            | TestStatus::TerminatedBySignal => num_skipped += 1,

            TestStatus::Failed {
                cached,
                exit_code: _,
            } => {
                num_failed += 1;
                if cached {
                    num_cached_results += 1;
                }
            }
            TestStatus::Passed {
                cached,
                fixed_tree_oid: _,
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
        "Tested {} with {}:",
        Pluralize {
            determiner: None,
            amount: results.len(),
            unit: ("commit", "commits")
        },
        effects.get_glyphs().render(
            StyledStringBuilder::new()
                .append_styled(command, Effect::Bold)
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

    if num_failed > 0 || num_skipped > 0 {
        Ok(ExitCode(1))
    } else {
        Ok(ExitCode(0))
    }
}

fn apply_fixes(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    dag: &Dag,
    repo: &Repo,
    event_log_db: &EventLogDb,
    execute_options: &ExecuteRebasePlanOptions,
    permissions: RebasePlanPermissions,
    command: &str,
    results: &[(NonZeroOid, TestOutput)],
) -> eyre::Result<ExitCode> {
    let fixed_tree_oids: Vec<(NonZeroOid, NonZeroOid)> = results
        .iter()
        .filter_map(|(commit_oid, test_output)| match test_output.test_status {
            TestStatus::Passed {
                cached: _,
                fixed_tree_oid: Some(fixed_tree_oid),
            } => Some((*commit_oid, fixed_tree_oid)),

            TestStatus::Passed {
                cached: _,
                fixed_tree_oid: None,
            }
            | TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::TerminatedBySignal
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_)
            | TestStatus::Failed { .. } => None,
        })
        .collect();

    let mut builder = RebasePlanBuilder::new(dag, permissions);
    let mut fixed_commit_oids = Vec::new();
    for (commit_oid, fixed_tree_oid) in fixed_tree_oids {
        let commit = repo.find_commit_or_fail(commit_oid)?;
        let fixed_tree = repo.find_tree_or_fail(fixed_tree_oid)?;
        if commit.get_tree_oid() == MaybeZeroOid::NonZero(fixed_tree.get_oid()) {
            warn!(
                ?commit_oid,
                ?fixed_tree_oid,
                "The result of fixing this commit produced the same tree OID as it already had"
            );
        }

        let commit_message = commit.get_message_raw()?;
        let commit_message = commit_message.to_str().with_context(|| {
            eyre::eyre!(
                "Could not decode commit message for commit: {:?}",
                commit_oid
            )
        })?;
        let parents: Vec<Commit> = commit
            .get_parent_oids()
            .into_iter()
            .map(|parent_oid| repo.find_commit_or_fail(parent_oid))
            .try_collect()?;
        let fixed_commit_oid = repo.create_commit(
            None,
            &commit.get_author(),
            &commit.get_committer(),
            commit_message,
            &fixed_tree,
            parents.iter().collect(),
        )?;
        fixed_commit_oids.push(fixed_commit_oid);
        builder.replace_commit(commit_oid, fixed_commit_oid)?;
        builder.move_subtree(commit_oid, commit.get_parent_oids())?;
    }

    let thread_pool = ThreadPoolBuilder::new().build()?;
    let repo_pool = RepoResource::new_pool(repo)?;
    let rebase_plan = match builder.build(effects, &thread_pool, &repo_pool)? {
        Ok(Some(plan)) => plan,
        Ok(None) => {
            writeln!(effects.get_output_stream(), "No commits to fix.")?;
            return Ok(ExitCode(0));
        }
        Err(err) => {
            err.describe(effects, repo)?;
            return Ok(ExitCode(1));
        }
    };

    match execute_rebase_plan(
        effects,
        git_run_info,
        repo,
        event_log_db,
        &rebase_plan,
        execute_options,
    )? {
        ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ } => {
            writeln!(
                effects.get_output_stream(),
                "Fixed {} with {}:",
                Pluralize {
                    determiner: None,
                    amount: fixed_commit_oids.len(),
                    unit: ("commit", "commits")
                },
                effects.get_glyphs().render(
                    StyledStringBuilder::new()
                        .append_styled(command, Effect::Bold)
                        .build()
                )?,
            )?;
            for fixed_commit_oid in fixed_commit_oids {
                let fixed_commit = repo.find_commit_or_fail(fixed_commit_oid)?;
                writeln!(
                    effects.get_output_stream(),
                    "{}",
                    effects
                        .get_glyphs()
                        .render(fixed_commit.friendly_describe(effects.get_glyphs())?)?
                )?;
            }

            Ok(ExitCode(0))
        }
        ExecuteRebasePlanResult::DeclinedToMerge { failed_merge_info } => {
            writeln!(effects.get_output_stream(), "BUG: encountered merge conflicts during git test fix, but we should not be applying any patches: {failed_merge_info:?}")?;
            Ok(ExitCode(1))
        }
        ExecuteRebasePlanResult::Failed { exit_code } => Ok(exit_code),
    }
}

type WorkerId = usize;
type Job = (NonZeroOid, OperationType);
type WorkQueue = Arc<Mutex<VecDeque<Job>>>;
enum JobResult {
    Done(NonZeroOid, TestOutput),
    Error(WorkerId, NonZeroOid, String),
}

fn worker(
    effects: &Effects,
    progress: &ProgressHandle,
    shell_path: &Path,
    git_run_info: &GitRunInfo,
    repo_dir: &Path,
    event_tx_id: EventTransactionId,
    options: &ResolvedTestOptions,
    worker_id: WorkerId,
    work_queue: WorkQueue,
    result_tx: Sender<JobResult>,
) {
    debug!(?worker_id, "Worker spawned");

    let repo = match Repo::from_dir(repo_dir) {
        Ok(repo) => repo,
        Err(err) => {
            panic!("Worker {worker_id} could not open repository at: {err}");
        }
    };

    let run_job = |job: Job| -> eyre::Result<bool> {
        let (commit_oid, operation_type) = job;
        let commit = repo.find_commit_or_fail(commit_oid)?;
        let test_output = run_test(
            effects,
            operation_type,
            git_run_info,
            shell_path,
            &repo,
            event_tx_id,
            options,
            worker_id,
            &commit,
        )?;
        progress.notify_progress_inc(1);

        debug!(?worker_id, ?commit_oid, "Worker sending Done job result");
        let should_terminate = result_tx
            .send(JobResult::Done(commit_oid, test_output))
            .is_err();
        debug!(
            ?worker_id,
            ?commit_oid,
            "Worker finished sending Done job result"
        );
        Ok(should_terminate)
    };

    while let Some(job) = {
        debug!(?worker_id, "Locking work queue");
        let mut work_queue = work_queue.lock().unwrap();
        debug!(?worker_id, "Locked work queue");
        let job = work_queue.pop_front();
        // Ensure we don't hold the lock while we process the job.
        drop(work_queue);
        debug!(?worker_id, "Unlocked work queue");
        job
    } {
        let (commit_oid, _) = job;
        debug!(?worker_id, ?commit_oid, "Worker accepted job");
        let job_result = run_job(job);
        debug!(?worker_id, ?commit_oid, "Worker finished job");
        match job_result {
            Ok(true) => break,
            Ok(false) => {
                // Continue.
            }
            Err(err) => {
                debug!(?worker_id, ?commit_oid, "Worker sending Error job result");
                result_tx
                    .send(JobResult::Error(worker_id, commit_oid, err.to_string()))
                    .ok();
                debug!(?worker_id, ?commit_oid, "Worker sending Error job result");
            }
        }
    }
    debug!(?worker_id, "Worker exiting");
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
        command: _,
        strategy,
        jobs: _, // Caller handles job management.
        verbosity: _,
        fix_options: _, // Checked in `test_commit`.
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
                *strategy,
                worker_id,
            )? {
                Err(_) => {
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
                Ok(PreparedWorkingDirectory {
                    mut lock_file,
                    path,
                }) => {
                    progress.notify_status(
                        OperationIcon::InProgress,
                        format!(
                            "Testing {}",
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
                    lock_file
                        .unlock()
                        .wrap_err_with(|| format!("Unlocking test dir at {path:?}"))?;
                    drop(lock_file);
                    result
                }
            }
        }
    };

    let description = StyledStringBuilder::new()
        .append(make_test_status_description(
            effects.get_glyphs(),
            commit,
            &test_output.test_status,
        )?)
        .build();
    progress.notify_status(
        match test_output.test_status {
            TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_) => OperationIcon::Warning,

            TestStatus::TerminatedBySignal | TestStatus::Failed { .. } => OperationIcon::Failure,

            TestStatus::Passed { .. } => OperationIcon::Success,
        },
        effects.get_glyphs().render(description)?,
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
    let tree_oid = commit.get_tree_oid();
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
                    fixed_tree_oid,
                }) => TestStatus::Passed {
                    cached: true,
                    fixed_tree_oid: fixed_tree_oid.map(|SerializedNonZeroOid(oid)| oid),
                },
                Ok(SerializedTestResult {
                    command: _,
                    exit_code,
                    fixed_tree_oid: _,
                }) => TestStatus::Failed {
                    cached: true,
                    exit_code,
                },
                Err(err) => TestStatus::ReadCacheFailed(err.to_string()),
            };
            return Ok(TestFilesResult::Cached(TestOutput {
                _result_path: result_path,
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
    let test_lock_dir_path = repo.get_test_dir().join("locks");
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
                // Don't show the `git checkout` operation among the progress bars, as we only want to see
                // the testing status.
                git_run_info.run_silent(
                    repo,
                    Some(event_tx_id),
                    &["checkout", &commit.get_oid().to_string()],
                    Default::default()
                )?;
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
            let parent_dir = repo.get_test_dir().join("worktrees");
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
        lock_file: _lock_file, // Make sure not to drop lock.
        result_path,
        result_file,
        stdout_path,
        stdout_file,
        stderr_path,
        stderr_file,
    } = test_files;
    let exit_code = match Command::new(shell_path)
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
        0 => {
            let fixed_tree_oid = match options.fix_options {
                None => None,
                Some(_) => {
                    let repo = Repo::from_dir(working_directory)?;
                    let index = repo.get_index()?;
                    let head_info = repo.get_head_info()?;
                    let effects = effects.suppress();
                    let (snapshot, _status) = repo.get_status(
                        &effects,
                        git_run_info,
                        &index,
                        &head_info,
                        Some(event_tx_id),
                    )?;
                    match snapshot.get_working_copy_changes_type()? {
                        WorkingCopyChangesType::None | WorkingCopyChangesType::Unstaged => {
                            let fixed_tree_oid = snapshot.commit_unstaged.get_tree_oid();
                            if commit.get_tree_oid() != fixed_tree_oid {
                                Option::<NonZeroOid>::from(snapshot.commit_unstaged.get_tree_oid())
                            } else {
                                None
                            }
                        }
                        WorkingCopyChangesType::Staged | WorkingCopyChangesType::Conflicts => {
                            // FIXME: surface information about the fix that failed to be applied.
                            None
                        }
                    }
                }
            };
            TestStatus::Passed {
                cached: false,
                fixed_tree_oid,
            }
        }
        exit_code => TestStatus::Failed {
            cached: false,
            exit_code,
        },
    };

    let serialized_test_result = SerializedTestResult {
        command: options.command.clone(),
        exit_code,
        fixed_tree_oid: match &test_status {
            TestStatus::Passed {
                cached: _,
                fixed_tree_oid,
            } => (*fixed_tree_oid).map(SerializedNonZeroOid),
            TestStatus::CheckoutFailed
            | TestStatus::SpawnTestFailed(_)
            | TestStatus::TerminatedBySignal
            | TestStatus::AlreadyInProgress
            | TestStatus::ReadCacheFailed(_)
            | TestStatus::Failed { .. } => None,
        },
    };
    serde_json::to_writer_pretty(result_file, &serialized_test_result)
        .wrap_err_with(|| format!("Writing test status {test_status:?} to {result_path:?}"))?;

    Ok(TestOutput {
        _result_path: result_path,
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
) -> eyre::Result<ExitCode> {
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
                return Ok(ExitCode(1));
            }
        };

    let options = match ResolvedTestOptions::resolve(
        now,
        effects,
        &dag,
        &repo,
        event_tx_id,
        &commit_set,
        None,
        options,
    )? {
        Ok(options) => options,
        Err(exit_code) => {
            return Ok(exit_code);
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

    Ok(ExitCode(0))
}

/// Delete cached test output for the commits in `revset`.
#[instrument]
pub fn subcommand_clean(
    effects: &Effects,
    revset: Revset,
    resolve_revset_options: &ResolveRevsetOptions,
) -> eyre::Result<ExitCode> {
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
        let tree_oid = commit.get_tree_oid();
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
