//! Phabricator backend for submitting patch stacks.

use std::collections::{HashMap, HashSet};
use std::fmt::{self, Debug, Display, Write};
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::time::SystemTime;

use cursive_core::theme::Effect;
use cursive_core::utils::markup::StyledString;
use git_branchless_opts::Revset;
use git_branchless_test::{
    run_tests, FixInfo, ResolvedTestOptions, TestOutput, TestResults, TestStatus,
    TestingAbortedError, Verbosity,
};
use itertools::Itertools;
use lazy_static::lazy_static;
use lib::core::check_out::CheckOutCommitOptions;
use lib::core::dag::{CommitSet, Dag};
use lib::core::effects::{Effects, OperationType, WithProgress};
use lib::core::eventlog::EventLogDb;
use lib::core::formatting::StyledStringBuilder;
use lib::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanError, BuildRebasePlanOptions, ExecuteRebasePlanOptions,
    ExecuteRebasePlanResult, RebasePlanBuilder, RebasePlanPermissions, RepoResource,
};
use lib::git::{
    Commit, GitRunInfo, MaybeZeroOid, NonZeroOid, Repo, RepoError, SignOption, TestCommand,
};
use lib::try_exit_code;
use lib::util::{ExitCode, EyreExitOr};
use rayon::ThreadPoolBuilder;
use regex::bytes::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{instrument, warn};

use crate::{CommitStatus, CreateStatus, Forge, SubmitOptions, SubmitStatus, STYLE_PUSHED};

/// Wrapper around the Phabricator "ID" type. (This is *not* a PHID, just a
/// regular ID).
#[derive(Clone, Debug, Serialize, Deserialize, Eq, Hash, PartialEq)]
#[serde(transparent)]
pub struct Id(pub String);

impl Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self(id) = self;
        write!(f, "D{id}")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, Hash, PartialEq)]
#[serde(transparent)]
struct Phid(pub String);

#[derive(Clone, Debug, Default, Serialize, Eq, PartialEq)]
struct DifferentialQueryRequest {
    ids: Vec<Id>,
    phids: Vec<Phid>,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
struct DifferentialEditRequest {
    #[serde(rename = "objectIdentifier")]
    id: Id, // could also be a PHID
    transactions: Vec<DifferentialEditTransaction>,
}

#[derive(Debug, Default, Serialize, Eq, PartialEq)]
struct DifferentialEditTransaction {
    r#type: String,
    value: Vec<Phid>,
}

#[derive(Debug, Deserialize)]
struct ConduitResponse<T> {
    #[serde(rename = "errorMessage")]
    error_message: Option<String>,
    response: Option<T>,
}

impl<T> ConduitResponse<T> {
    fn check_err(self) -> std::result::Result<T, String> {
        let Self {
            error_message,
            response,
        } = self;
        match error_message {
            Some(error_message) => Err(error_message),
            None => match response {
                None => Err("(no error message)".to_string()),
                Some(response) => Ok(response),
            },
        }
    }
}

impl<T: Default> Default for ConduitResponse<T> {
    fn default() -> Self {
        Self {
            error_message: Default::default(),
            response: Default::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct DifferentialQueryRevisionResponse {
    id: Id,
    phid: Phid,

    #[serde(default)]
    hashes: Vec<(String, String)>,

    #[serde(default)]
    auxiliary: DifferentialQueryAuxiliaryResponse,
}

#[derive(Debug, Default, Deserialize)]
struct DifferentialQueryAuxiliaryResponse {
    // TODO: add `default`
    #[serde(rename = "phabricator:depends-on")]
    phabricator_depends_on: Vec<Phid>,
}

/// Error type.
#[allow(missing_docs)]
#[derive(Debug, Error)]
pub enum Error {
    #[error("no working copy for repository at path: {}", .repo_path.display())]
    NoWorkingCopy { repo_path: PathBuf },

    #[error("could not iterate commits: {0}")]
    IterCommits(#[source] eyre::Error),

    #[error("could not look up commits: {0}")]
    LookUpCommits(#[source] RepoError),

    #[error("no commit with hash {commit_oid:?}: {source}")]
    NoSuchCommit {
        source: RepoError,
        commit_oid: NonZeroOid,
    },

    #[error("invocation to `arc {args}` failed: {source}", args = args.join(" "))]
    InvokeArc {
        source: io::Error,
        args: Vec<String>,
    },

    #[error("communication with `arc {args}` failed: {source}", args = args.join(" "))]
    CommunicateWithArc {
        source: serde_json::Error,
        args: Vec<String>,
    },

    #[error("could not create phab for {commit_oid} when running `arc {args}` (exit code {exit_code}): {message}", args = args.join(" "))]
    CreatePhab {
        exit_code: i32,
        message: String,
        commit_oid: NonZeroOid,
        args: Vec<String>,
    },

    #[error("could not query dependencies when running `arc {args}` (exit code {exit_code}): {message}", args = args.join(" "))]
    QueryDependencies {
        exit_code: i32,
        message: String,
        args: Vec<String>,
    },

    #[error("could not update dependencies when running `arc {args}` (exit code {exit_code}): {message}", args = args.join(" "))]
    UpdateDependencies {
        exit_code: i32,
        message: String,
        args: Vec<String>,
    },

    #[error("could not parse response when running `arc {args}`: {source}; with output: {output}", args = args.join(" "))]
    ParseResponse {
        source: serde_json::Error,
        output: String,
        args: Vec<String>,
    },

    #[error("error when calling Conduit API with request {request:?}: {message}")]
    Conduit {
        request: Box<dyn Debug + Send + Sync>,
        message: String,
    },

    #[error("could not make transaction ID: {source}")]
    MakeTransactionId { source: eyre::Error },

    #[error("could not execute `arc diff` on commits: {source}")]
    ExecuteArcDiff { source: eyre::Error },

    #[error("could not verify permissions to rewrite commits: {source}")]
    VerifyPermissions { source: eyre::Error },

    #[error("could not build rebase plan")]
    BuildRebasePlan(BuildRebasePlanError),

    #[error("failed to rewrite commits with exit code {}", exit_code.0)]
    RewriteCommits { exit_code: ExitCode },

    #[error(transparent)]
    Fmt(#[from] fmt::Error),

    #[error(transparent)]
    DagError(#[from] eden_dag::Error),
}

/// Result type.
pub type Result<T> = std::result::Result<T, Error>;

/// When this environment variable is set, the implementation of the Phabricator
/// forge will make mock calls instead of actually invoking `arc`.
pub const SHOULD_MOCK_ENV_KEY: &str = "BRANCHLESS_SUBMIT_PHABRICATOR_MOCK";

fn should_mock() -> bool {
    std::env::var_os(SHOULD_MOCK_ENV_KEY).is_some()
}

/// The [Phabricator](https://en.wikipedia.org/wiki/Phabricator) code review system.
///
/// Note that Phabricator is no longer actively maintained, but many
/// organizations still use it.
#[allow(missing_docs)]
#[derive(Debug)]
pub struct PhabricatorForge<'a> {
    pub effects: &'a Effects,
    pub git_run_info: &'a GitRunInfo,
    pub repo: &'a Repo,
    pub dag: &'a mut Dag,
    pub event_log_db: &'a EventLogDb<'a>,
    pub revset: &'a Revset,
}

impl Forge for PhabricatorForge<'_> {
    #[instrument]
    fn query_status(
        &mut self,
        commit_set: CommitSet,
    ) -> eyre::Result<std::result::Result<HashMap<NonZeroOid, CommitStatus>, ExitCode>> {
        let commit_oids = self.dag.commit_set_to_vec(&commit_set)?;
        let commit_oid_to_revision: HashMap<NonZeroOid, Option<Id>> = commit_oids
            .into_iter()
            .map(|commit_oid| -> eyre::Result<_> {
                let revision_id = self.get_revision_id(commit_oid)?;
                Ok((commit_oid, revision_id))
            })
            .try_collect()?;

        let revisions = if should_mock() {
            Default::default()
        } else {
            self.query_revisions(&DifferentialQueryRequest {
                ids: commit_oid_to_revision.values().flatten().cloned().collect(),
                phids: Default::default(),
            })?
        };
        let commit_hashes: HashMap<Id, NonZeroOid> = revisions
            .into_iter()
            .filter_map(|item| {
                let hashes: HashMap<String, String> = item.hashes.iter().cloned().collect();
                if hashes.is_empty() {
                    None
                } else {
                    // `gtcm` stands for "git commit" (as opposed to `gttr`, also returned in the same list, or `hgcm`, which stands for "hg commit").
                    match hashes.get("gtcm") {
                        None => {
                            warn!(?item, "No Git commit hash in item");
                            None
                        }
                        Some(commit_oid) => match NonZeroOid::from_str(commit_oid.as_str()) {
                            Ok(commit_oid) => Some((item.id, commit_oid)),
                            Err(err) => {
                                warn!(?err, "Couldn't parse Git commit OID");
                                None
                            }
                        },
                    }
                }
            })
            .collect();

        let statuses = commit_oid_to_revision
            .into_iter()
            .map(|(commit_oid, id)| {
                let status = CommitStatus {
                    submit_status: match id {
                        Some(id) => match commit_hashes.get(&id) {
                            Some(remote_commit_oid) => {
                                if remote_commit_oid == &commit_oid {
                                    SubmitStatus::UpToDate
                                } else {
                                    SubmitStatus::NeedsUpdate
                                }
                            }
                            None => {
                                warn!(?commit_oid, ?id, "No remote commit hash found for commit");
                                SubmitStatus::NeedsUpdate
                            }
                        },
                        None => SubmitStatus::Unsubmitted,
                    },
                    remote_name: None,
                    local_commit_name: None,
                    remote_commit_name: None,
                };
                (commit_oid, status)
            })
            .collect();
        Ok(Ok(statuses))
    }

    #[instrument]
    fn create(
        &mut self,
        commits: HashMap<NonZeroOid, CommitStatus>,
        options: &SubmitOptions,
    ) -> eyre::Result<std::result::Result<HashMap<NonZeroOid, CreateStatus>, ExitCode>> {
        let SubmitOptions {
            create: _,
            draft,
            execution_strategy,
            num_jobs,
            message: _,
        } = options;

        let commit_set = commits.keys().copied().collect();
        let commit_oids = self.dag.sort(&commit_set).map_err(Error::IterCommits)?;
        let commits: Vec<Commit> = commit_oids
            .iter()
            .map(|commit_oid| self.repo.find_commit_or_fail(*commit_oid))
            .collect::<std::result::Result<_, _>>()
            .map_err(Error::LookUpCommits)?;
        let now = SystemTime::now();
        let event_tx_id = self
            .event_log_db
            .make_transaction_id(now, "phabricator create")
            .map_err(|err| Error::MakeTransactionId { source: err })?;
        let build_options = BuildRebasePlanOptions {
            force_rewrite_public_commits: false,
            dump_rebase_constraints: false,
            dump_rebase_plan: false,
            detect_duplicate_commits_via_patch_id: false,
        };
        let execute_options = ExecuteRebasePlanOptions {
            now,
            event_tx_id,
            preserve_timestamps: true,
            force_in_memory: true,
            force_on_disk: false,
            resolve_merge_conflicts: false,
            check_out_commit_options: CheckOutCommitOptions {
                render_smartlog: false,
                ..Default::default()
            },
            sign_option: SignOption::Disable,
        };
        let permissions =
            RebasePlanPermissions::verify_rewrite_set(self.dag, build_options, &commit_set)
                .map_err(|err| Error::VerifyPermissions { source: err })?
                .map_err(Error::BuildRebasePlan)?;
        let command = if !should_mock() {
            let mut args = vec!["arc", "diff", "--create", "--verbatim", "--allow-untracked"];
            if *draft {
                args.push("--draft");
            }
            args.extend(["--", "HEAD^"]);
            TestCommand::Args(args.into_iter().map(ToString::to_string).collect())
        } else {
            TestCommand::String(
                r#"git commit --amend --message "$(git show --no-patch --format=%B HEAD)

Differential Revision: https://phabricator.example.com/D000$(git rev-list --count HEAD)
            "
            "#
                .to_string(),
            )
        };

        let test_results = match run_tests(
            now,
            self.effects,
            self.git_run_info,
            self.dag,
            self.repo,
            self.event_log_db,
            self.revset,
            &commits,
            &ResolvedTestOptions {
                command,
                execution_strategy: *execution_strategy,
                search_strategy: None,
                is_dry_run: false,
                use_cache: false,
                is_interactive: false,
                num_jobs: *num_jobs,
                verbosity: Verbosity::None,
                fix_options: Some((execute_options.clone(), permissions.clone())),
            },
        ) {
            Ok(Ok(test_results)) => test_results,
            Ok(Err(exit_code)) => return Ok(Err(exit_code)),
            Err(err) => return Err(Error::ExecuteArcDiff { source: err }.into()),
        };

        let TestResults {
            search_bounds: _,
            test_outputs,
            testing_aborted_error,
        } = test_results;
        if let Some(testing_aborted_error) = testing_aborted_error {
            let TestingAbortedError {
                commit_oid,
                exit_code,
            } = testing_aborted_error;
            writeln!(
                self.effects.get_output_stream(),
                "Uploading was aborted with exit code {exit_code} due to commit {}",
                self.effects.get_glyphs().render(
                    self.repo
                        .friendly_describe_commit_from_oid(self.effects.get_glyphs(), commit_oid)?
                )?,
            )?;
            return Ok(Err(ExitCode(1)));
        }

        let rebase_plan = {
            let mut builder = RebasePlanBuilder::new(self.dag, permissions);
            for (commit_oid, test_output) in test_outputs {
                let head_commit_oid = match test_output.test_status {
                    TestStatus::CheckoutFailed
                    | TestStatus::SpawnTestFailed(_)
                    | TestStatus::TerminatedBySignal
                    | TestStatus::AlreadyInProgress
                    | TestStatus::ReadCacheFailed(_)
                    | TestStatus::Indeterminate { .. }
                    | TestStatus::Abort { .. }
                    | TestStatus::Failed { .. } => {
                        self.render_failed_test(commit_oid, &test_output)?;
                        return Ok(Err(ExitCode(1)));
                    }
                    TestStatus::Passed {
                        cached: _,
                        fix_info:
                            FixInfo {
                                head_commit_oid,
                                snapshot_tree_oid: _,
                            },
                        interactive: _,
                    } => head_commit_oid,
                };

                let commit = self.repo.find_commit_or_fail(commit_oid)?;
                builder.move_subtree(commit.get_oid(), commit.get_parent_oids())?;
                builder.replace_commit(commit.get_oid(), head_commit_oid.unwrap_or(commit_oid))?;
            }

            let pool = ThreadPoolBuilder::new().build()?;
            let repo_pool = RepoResource::new_pool(self.repo)?;
            match builder.build(self.effects, &pool, &repo_pool)? {
                Ok(Some(rebase_plan)) => rebase_plan,
                Ok(None) => return Ok(Ok(Default::default())),
                Err(err) => {
                    err.describe(self.effects, self.repo, self.dag)?;
                    return Ok(Err(ExitCode(1)));
                }
            }
        };

        let rewritten_oids = match execute_rebase_plan(
            self.effects,
            self.git_run_info,
            self.repo,
            self.event_log_db,
            &rebase_plan,
            &execute_options,
        )? {
            ExecuteRebasePlanResult::Succeeded {
                rewritten_oids: Some(rewritten_oids),
            } => rewritten_oids,
            ExecuteRebasePlanResult::Succeeded {
                rewritten_oids: None,
            } => {
                warn!("No rewritten commit OIDs were produced by rebase plan execution");
                Default::default()
            }
            ExecuteRebasePlanResult::DeclinedToMerge {
                failed_merge_info: _,
            } => {
                writeln!(
                    self.effects.get_error_stream(),
                    "BUG: Merge failed, but rewording shouldn't cause any merge failures."
                )?;
                return Ok(Err(ExitCode(1)));
            }
            ExecuteRebasePlanResult::Failed { exit_code } => {
                return Ok(Err(exit_code));
            }
        };

        let mut create_statuses = HashMap::new();
        for commit_oid in commit_oids {
            let final_commit_oid = match rewritten_oids.get(&commit_oid) {
                Some(MaybeZeroOid::NonZero(commit_oid)) => *commit_oid,
                Some(MaybeZeroOid::Zero) => {
                    warn!(?commit_oid, "Commit was rewritten to the zero OID",);
                    commit_oid
                }
                None => commit_oid,
            };
            let local_branch_name = {
                match self.get_revision_id(final_commit_oid)? {
                    Some(Id(id)) => format!("D{id}"),
                    None => {
                        writeln!(
                            self.effects.get_output_stream(),
                            "Failed to upload (link to newly-created revision not found in commit message): {}",
                            self.effects.get_glyphs().render(
                                self.repo.friendly_describe_commit_from_oid(
                                    self.effects.get_glyphs(),
                                    final_commit_oid
                                )?
                            )?,
                        )?;
                        return Ok(Err(ExitCode(1)));
                    }
                }
            };
            create_statuses.insert(
                commit_oid,
                CreateStatus {
                    final_commit_oid,
                    local_commit_name: local_branch_name,
                },
            );
        }

        let final_commit_oids: CommitSet = create_statuses
            .values()
            .map(|create_status| {
                let CreateStatus {
                    final_commit_oid,
                    local_commit_name: _,
                } = create_status;
                *final_commit_oid
            })
            .collect();
        self.dag.sync_from_oids(
            self.effects,
            self.repo,
            CommitSet::empty(),
            final_commit_oids.clone(),
        )?;
        match self.update_dependencies(&final_commit_oids, &final_commit_oids)? {
            Ok(()) => {}
            Err(exit_code) => return Ok(Err(exit_code)),
        }

        Ok(Ok(create_statuses))
    }

    #[instrument]
    fn update(
        &mut self,
        commits: HashMap<NonZeroOid, crate::CommitStatus>,
        options: &SubmitOptions,
    ) -> EyreExitOr<()> {
        let SubmitOptions {
            create: _,
            draft: _,
            execution_strategy,
            num_jobs,
            message,
        } = options;

        let commit_set = commits.keys().copied().collect();
        // Sort for consistency with `update_dependencies`.
        let commit_oids = self.dag.sort(&commit_set)?;
        let commits: Vec<_> = commit_oids
            .into_iter()
            .map(|commit_oid| self.repo.find_commit_or_fail(commit_oid))
            .try_collect()?;

        let now = SystemTime::now();
        let event_tx_id = self
            .event_log_db
            .make_transaction_id(now, "phabricator update")?;
        let build_options = BuildRebasePlanOptions {
            force_rewrite_public_commits: false,
            dump_rebase_constraints: false,
            dump_rebase_plan: false,
            detect_duplicate_commits_via_patch_id: false,
        };
        let execute_options = ExecuteRebasePlanOptions {
            now,
            event_tx_id,
            preserve_timestamps: true,
            force_in_memory: true,
            force_on_disk: false,
            resolve_merge_conflicts: false,
            check_out_commit_options: CheckOutCommitOptions {
                render_smartlog: false,
                ..Default::default()
            },
            sign_option: SignOption::Disable,
        };
        let permissions =
            RebasePlanPermissions::verify_rewrite_set(self.dag, build_options, &commit_set)
                .map_err(|err| Error::VerifyPermissions { source: err })?
                .map_err(Error::BuildRebasePlan)?;
        let test_options = ResolvedTestOptions {
            command: if !should_mock() {
                let mut args = vec![
                    "arc",
                    "diff",
                    "--head",
                    "HEAD",
                    "HEAD^",
                    "--allow-untracked",
                ];
                args.extend(match message {
                    Some(message) => ["-m", message.as_ref()],
                    None => ["-m", "update"],
                });
                TestCommand::Args(args.into_iter().map(ToString::to_string).collect())
            } else {
                TestCommand::String("echo Submitting $(git rev-parse HEAD)".to_string())
            },
            execution_strategy: *execution_strategy,
            search_strategy: None,
            is_dry_run: false,
            use_cache: false,
            is_interactive: false,
            num_jobs: *num_jobs,
            verbosity: Verbosity::None,
            fix_options: Some((execute_options, permissions)),
        };
        let TestResults {
            search_bounds: _,
            test_outputs,
            testing_aborted_error,
        } = try_exit_code!(run_tests(
            now,
            self.effects,
            self.git_run_info,
            self.dag,
            self.repo,
            self.event_log_db,
            self.revset,
            &commits,
            &test_options,
        )?);
        if let Some(testing_aborted_error) = testing_aborted_error {
            let TestingAbortedError {
                commit_oid,
                exit_code,
            } = testing_aborted_error;
            writeln!(
                self.effects.get_output_stream(),
                "Updating was aborted with exit code {exit_code} due to commit {}",
                self.effects.get_glyphs().render(
                    self.repo
                        .friendly_describe_commit_from_oid(self.effects.get_glyphs(), commit_oid)?
                )?,
            )?;
            return Ok(Err(ExitCode(1)));
        }

        let (success_commits, failure_commits): (Vec<_>, Vec<_>) = test_outputs
            .into_iter()
            .partition(|(_commit_oid, test_output)| match test_output.test_status {
                TestStatus::Passed { .. } => true,
                TestStatus::CheckoutFailed
                | TestStatus::SpawnTestFailed(_)
                | TestStatus::TerminatedBySignal
                | TestStatus::AlreadyInProgress
                | TestStatus::ReadCacheFailed(_)
                | TestStatus::Indeterminate { .. }
                | TestStatus::Abort { .. }
                | TestStatus::Failed { .. } => false,
            });
        if !failure_commits.is_empty() {
            let effects = self.effects;
            writeln!(
                effects.get_output_stream(),
                "Failed when running command: {}",
                effects.get_glyphs().render(
                    StyledStringBuilder::new()
                        .append_styled(test_options.command.to_string(), Effect::Bold)
                        .build()
                )?
            )?;
            for (commit_oid, test_output) in failure_commits {
                self.render_failed_test(commit_oid, &test_output)?;
            }
            return Ok(Err(ExitCode(1)));
        }

        try_exit_code!(self.update_dependencies(
            &success_commits
                .into_iter()
                .map(|(commit_oid, _test_output)| commit_oid)
                .collect(),
            &CommitSet::empty()
        )?);
        Ok(Ok(()))
    }
}

impl PhabricatorForge<'_> {
    fn query_revisions(
        &self,
        request: &DifferentialQueryRequest,
    ) -> Result<Vec<DifferentialQueryRevisionResponse>> {
        // The API call seems to hang if we don't specify any IDs; perhaps it's
        // fetching everything?
        if request == &DifferentialQueryRequest::default() {
            return Ok(Default::default());
        }

        let args = vec![
            "call-conduit".to_string(),
            "--".to_string(),
            "differential.query".to_string(),
        ];
        let mut child = Command::new("arc")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| Error::InvokeArc {
                source: err,
                args: args.clone(),
            })?;
        serde_json::to_writer_pretty(child.stdin.take().unwrap(), request).map_err(|err| {
            Error::CommunicateWithArc {
                source: err,
                args: args.clone(),
            }
        })?;
        let result = child.wait_with_output().map_err(|err| Error::InvokeArc {
            source: err,
            args: args.clone(),
        })?;
        if !result.status.success() {
            return Err(Error::QueryDependencies {
                exit_code: result.status.code().unwrap_or(-1),
                message: String::from_utf8_lossy(&result.stdout).into_owned(),
                args,
            });
        }

        let output: ConduitResponse<Vec<DifferentialQueryRevisionResponse>> =
            serde_json::from_slice(&result.stdout).map_err(|err| Error::ParseResponse {
                source: err,
                output: String::from_utf8_lossy(&result.stdout).into_owned(),
                args: args.clone(),
            })?;
        let response = output.check_err().map_err(|message| Error::Conduit {
            request: Box::new(request.clone()),
            message,
        })?;
        Ok(response)
    }

    /// Query the dependencies of a set of commits from Phabricator (not locally).
    pub fn query_remote_dependencies(
        &self,
        commit_oids: HashSet<NonZeroOid>,
    ) -> Result<HashMap<NonZeroOid, HashSet<NonZeroOid>>> {
        // Convert commit hashes to IDs.
        let commit_oid_to_id: HashMap<NonZeroOid, Option<Id>> = {
            let mut result = HashMap::new();
            for commit_oid in commit_oids.iter().copied() {
                let revision_id = self.get_revision_id(commit_oid)?;
                result.insert(commit_oid, revision_id);
            }
            result
        };

        // Get the reverse mapping of IDs to commit hashes. Note that not every commit
        // hash will have an ID -- specifically those which haven't been submitted yet.
        let id_to_commit_oid: HashMap<Id, NonZeroOid> = commit_oid_to_id
            .iter()
            .filter_map(|(commit_oid, id)| id.as_ref().map(|v| (v.clone(), *commit_oid)))
            .collect();

        // Query for revision information by ID.
        let query_ids: Vec<Id> = commit_oid_to_id
            .values()
            .filter_map(|id| id.as_ref().cloned())
            .collect();

        let revisions = self.query_revisions(&DifferentialQueryRequest {
            ids: query_ids,
            phids: Default::default(),
        })?;

        // Get the dependency PHIDs for each revision ID.
        let dependency_phids: HashMap<Id, Vec<Phid>> = revisions
            .into_iter()
            .map(|revision| {
                let DifferentialQueryRevisionResponse {
                    id,
                    phid: _,
                    hashes: _,
                    auxiliary:
                        DifferentialQueryAuxiliaryResponse {
                            phabricator_depends_on,
                        },
                } = revision;
                (id, phabricator_depends_on)
            })
            .collect();

        // Convert the dependency PHIDs back into revision IDs.
        let dependency_ids: HashMap<Id, Vec<Id>> = {
            let all_phids: Vec<Phid> = dependency_phids.values().flatten().cloned().collect();
            let revisions = self.query_revisions(&DifferentialQueryRequest {
                ids: Default::default(),
                phids: all_phids,
            })?;
            let phid_to_id: HashMap<Phid, Id> = revisions
                .into_iter()
                .map(|revision| {
                    let DifferentialQueryRevisionResponse {
                        id,
                        phid,
                        hashes: _,
                        auxiliary: _,
                    } = revision;
                    (phid, id)
                })
                .collect();
            dependency_phids
                .into_iter()
                .map(|(id, dependency_phids)| {
                    (
                        id,
                        dependency_phids
                            .into_iter()
                            .filter_map(|dependency_phid| phid_to_id.get(&dependency_phid))
                            .cloned()
                            .collect(),
                    )
                })
                .collect()
        };

        // Use the looked-up IDs to convert the commit dependencies. Note that
        // there may be dependencies not expressed in the set of commits, in
        // which case... FIXME.
        let result: HashMap<NonZeroOid, HashSet<NonZeroOid>> = commit_oid_to_id
            .into_iter()
            .map(|(commit_oid, id)| {
                let dependency_ids = match id {
                    None => Default::default(),
                    Some(id) => match dependency_ids.get(&id) {
                        None => Default::default(),
                        Some(dependency_ids) => dependency_ids
                            .iter()
                            .filter_map(|dependency_id| id_to_commit_oid.get(dependency_id))
                            .copied()
                            .collect(),
                    },
                };
                (commit_oid, dependency_ids)
            })
            .collect();
        Ok(result)
    }

    fn update_dependencies(
        &self,
        commits: &CommitSet,
        newly_created_commits: &CommitSet,
    ) -> eyre::Result<std::result::Result<(), ExitCode>> {
        // Make sure to update dependencies in topological order to prevent
        // dependency cycles.
        let commit_oids = self.dag.sort(commits)?;

        let (effects, progress) = self.effects.start_operation(OperationType::UpdateCommits);

        // Newly-created commits won't have been observed by the DAG, so add them in manually here.
        let draft_commits = self.dag.query_draft_commits()?.union(newly_created_commits);

        for commit_oid in commit_oids.into_iter().with_progress(progress) {
            let id = match self.get_revision_id(commit_oid)? {
                Some(id) => id,
                None => {
                    warn!(?commit_oid, "No Phabricator commit ID for latest commit");
                    continue;
                }
            };
            let commit = self.repo.find_commit_or_fail(commit_oid)?;
            let parent_oids = commit.get_parent_oids();

            let mut parent_revision_ids = Vec::new();
            for parent_oid in parent_oids {
                if !self.dag.set_contains(&draft_commits, parent_oid)? {
                    // FIXME: this will exclude commits that used to be part of
                    // the stack but have since landed.
                    continue;
                }
                let parent_revision_id = match self.get_revision_id(parent_oid)? {
                    Some(id) => id,
                    None => continue,
                };
                parent_revision_ids.push(parent_revision_id);
            }

            let id_str = effects.get_glyphs().render(Self::render_id(&id))?;
            if parent_revision_ids.is_empty() {
                writeln!(
                    effects.get_output_stream(),
                    "Setting {id_str} as stack root (no dependencies)",
                )?;
            } else {
                writeln!(
                    effects.get_output_stream(),
                    "Stacking {id_str} on top of {}",
                    effects.get_glyphs().render(StyledStringBuilder::join(
                        ", ",
                        parent_revision_ids.iter().map(Self::render_id).collect()
                    ))?,
                )?;
            }

            match self.set_dependencies(id, parent_revision_ids)? {
                Ok(()) => {}
                Err(exit_code) => return Ok(Err(exit_code)),
            }
        }
        Ok(Ok(()))
    }

    fn render_id(id: &Id) -> StyledString {
        StyledStringBuilder::new()
            .append_styled(id.to_string(), *STYLE_PUSHED)
            .build()
    }

    fn set_dependencies(
        &self,
        id: Id,
        parent_revision_ids: Vec<Id>,
    ) -> eyre::Result<std::result::Result<(), ExitCode>> {
        let effects = self.effects;

        if should_mock() {
            return Ok(Ok(()));
        }

        let revisions = self.query_revisions(&DifferentialQueryRequest {
            ids: parent_revision_ids,
            phids: Default::default(),
        })?;
        let parent_revision_phids: Vec<Phid> = revisions
            .into_iter()
            .map(|response| response.phid)
            .collect();
        let request = DifferentialEditRequest {
            id,
            transactions: vec![DifferentialEditTransaction {
                r#type: "parents.set".to_string(),
                value: parent_revision_phids,
            }],
        };

        let args = vec![
            "call-conduit".to_string(),
            "--".to_string(),
            "differential.revision.edit".to_string(),
        ];
        let mut child = Command::new("arc")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| Error::InvokeArc {
                source: err,
                args: args.clone(),
            })?;
        serde_json::to_writer_pretty(child.stdin.take().unwrap(), &request).map_err(|err| {
            Error::CommunicateWithArc {
                source: err,
                args: args.clone(),
            }
        })?;
        let result = child.wait_with_output().map_err(|err| Error::InvokeArc {
            source: err,
            args: args.clone(),
        })?;
        if !result.status.success() {
            let args = args.join(" ");
            let exit_code = ExitCode::try_from(result.status)?;
            let ExitCode(exit_code_isize) = exit_code;
            writeln!(
                effects.get_output_stream(),
                "Could not update dependencies when running `arc {args}` (exit code {exit_code_isize}):",
            )?;
            writeln!(
                effects.get_output_stream(),
                "{}",
                String::from_utf8_lossy(&result.stdout)
            )?;
            return Ok(Err(exit_code));
        }

        Ok(Ok(()))
    }

    /// Given a commit for D123, returns a string like "123" by parsing the
    /// commit message.
    pub fn get_revision_id(&self, commit_oid: NonZeroOid) -> Result<Option<Id>> {
        let commit =
            self.repo
                .find_commit_or_fail(commit_oid)
                .map_err(|err| Error::NoSuchCommit {
                    source: err,
                    commit_oid,
                })?;
        let message = commit.get_message_raw();

        lazy_static! {
            static ref RE: Regex = Regex::new(
                r"(?mx)
^
Differential[\ ]Revision:[\ ]
    (.+ /)?
    D(?P<diff>[0-9]+)
$",
            )
            .expect("Failed to compile `extract_diff_number` regex");
        }
        let captures = match RE.captures(message.as_slice()) {
            Some(captures) => captures,
            None => return Ok(None),
        };
        let diff_number = &captures["diff"];
        let diff_number = String::from_utf8(diff_number.to_vec())
            .expect("Regex should have confirmed that this string was only ASCII digits");
        Ok(Some(Id(diff_number)))
    }

    fn render_failed_test(
        &self,
        commit_oid: NonZeroOid,
        test_output: &TestOutput,
    ) -> eyre::Result<()> {
        let commit = self.repo.find_commit_or_fail(commit_oid)?;
        writeln!(
            self.effects.get_output_stream(),
            "{}",
            self.effects
                .get_glyphs()
                .render(test_output.test_status.describe(
                    self.effects.get_glyphs(),
                    &commit,
                    false
                )?)?,
        )?;
        let stdout = std::fs::read_to_string(&test_output.stdout_path)?;
        write!(self.effects.get_output_stream(), "Stdout:\n{stdout}")?;
        let stderr = std::fs::read_to_string(&test_output.stderr_path)?;
        write!(self.effects.get_output_stream(), "Stderr:\n{stderr}")?;
        Ok(())
    }
}
