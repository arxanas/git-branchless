//! Push the user's commits to a remote.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

mod branch_forge;
pub mod github;
pub mod phabricator;

use std::collections::HashMap;
use std::fmt::{Debug, Write};
use std::time::SystemTime;

use branch_forge::BranchForge;
use cursive_core::theme::{BaseColor, Effect, Style};
use cursive_core::utils::markup::StyledString;
use git_branchless_invoke::CommandContext;
use git_branchless_test::{RawTestOptions, ResolvedTestOptions, Verbosity};
use github::GithubForge;
use itertools::Itertools;
use lazy_static::lazy_static;
use lib::core::dag::{union_all, CommitSet, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::repo_ext::{RepoExt, RepoReferencesSnapshot};
use lib::git::{GitRunInfo, NonZeroOid, Repo};
use lib::try_exit_code;
use lib::util::{ExitCode, EyreExitOr};

use git_branchless_opts::{
    ForgeKind, ResolveRevsetOptions, Revset, SubmitArgs, TestExecutionStrategy,
};
use git_branchless_revset::resolve_commits;
use phabricator::PhabricatorForge;
use tracing::{debug, info, instrument, warn};

use crate::github::github_push_remote;

lazy_static! {
    /// The style for commits which were successfully submitted.
    pub static ref STYLE_SUCCESS: Style =
        Style::merge(&[BaseColor::Green.light().into(), Effect::Bold.into()]);

    /// The style for commits which were not submitted.
    pub static ref STYLE_SKIPPED: Style =
        Style::merge(&[BaseColor::Yellow.light().into(), Effect::Bold.into()]);

    /// The style for commits which failed to be submitted.
    pub static ref STYLE_FAILED: Style =
        Style::merge(&[BaseColor::Red.light().into(), Effect::Bold.into()]);

    /// The style for informational text.
    pub static ref STYLE_INFO: Style = Effect::Dim.into();
}

/// The status of a commit, indicating whether it needs to be updated remotely.
#[derive(Clone, Debug)]
pub enum SubmitStatus {
    /// The commit exists locally and there is no intention to push it to the
    /// remote.
    Local,

    /// The commit exists locally and will eventually be pushed to the remote,
    /// but it has not been pushed yet.
    Unsubmitted,

    /// It could not be determined whether the remote commit exists.
    Unknown,

    /// The same commit exists both locally and remotely.
    UpToDate,

    /// The commit exists locally but is associated with a different remote
    /// commit, so it needs to be updated.
    NeedsUpdate,
}

/// Information about each commit.
#[derive(Clone, Debug)]
pub struct CommitStatus {
    /// The status of this commit, indicating whether it needs to be updated.
    submit_status: SubmitStatus,

    /// The Git remote associated with this commit, if any.
    remote_name: Option<String>,

    /// An identifier corresponding to the commit in the local repository. This
    /// may be a branch name, a change ID, the commit summary, etc.
    ///
    /// This does not necessarily correspond to the commit's name/identifier in
    /// the forge (e.g. not a code review link); see `remote_commit_name`
    /// instead.
    ///
    /// The calling code will only use this for display purposes, but an
    /// individual forge implementation can return this from
    /// `Forge::query_status` and it will be passed back to
    /// `Forge::create`/`Forge::update`.
    local_commit_name: Option<String>,

    /// An identifier corresponding to the commit in the remote repository. This
    /// may be a branch name, a change ID, a code review link, etc.
    ///
    /// This does not necessarily correspond to the commit's name/identifier in
    /// the local repository; see `local_commit_name` instead.
    ///
    /// The calling code will only use this for display purposes, but an
    /// individual forge implementation can return this from
    /// `Forge::query_status` and it will be passed back to
    /// `Forge::create`/`Forge::update`.
    remote_commit_name: Option<String>,
}

/// Options for submitting commits to the forge.
#[derive(Clone, Debug)]
pub struct SubmitOptions {
    /// Create associated branches, code reviews, etc. for each of the provided commits.
    ///
    /// This should be an idempotent behavior, i.e. setting `create` to `true`
    /// and submitting a commit which already has an associated remote item
    /// should not have any additional effect.
    pub create: bool,

    /// When creating new code reviews for the currently-submitting commits,
    /// configure those code reviews to indicate that they are not yet ready for
    /// review.
    ///
    /// If a "draft" state is not meaningful for the forge, then has no effect.
    /// If a given commit is already submitted, then has no effect for that
    /// commit's code review.
    pub draft: bool,

    /// For implementations which need to use the working copy to create the
    /// code review, the appropriate execution strategy to do so.
    pub execution_strategy: TestExecutionStrategy,

    /// The number of jobs to use when submitting commits.
    pub num_jobs: usize,

    /// An optional message to include with the create or update operation.
    pub message: Option<String>,
}

/// The result of creating a commit.
#[derive(Clone, Debug)]
pub enum CreateStatus {
    /// The commit was successfully created.
    Created {
        /// The commit OID after carrying out the creation process. Usually, this
        /// will be the same as the original commit OID, unless the forge amends it
        /// (e.g. to include a change ID).
        final_commit_oid: NonZeroOid,

        /// An identifier corresponding to the commit, for display purposes only.
        /// This may be a branch name, a change ID, the commit summary, etc.
        ///
        /// This does not necessarily correspond to the commit's name/identifier in
        /// the forge (e.g. not a code review link).
        local_commit_name: String,
    },

    /// The commit was skipped because it didn't already exist on the remote and
    /// `--create` was not passed.
    Skipped,

    /// The commit could not be created due to an error.
    Err {
        /// The error message to display to the user.
        message: String,
    },
}

/// The result of updating a commit.
///
/// TODO: have an `Err` case.
#[derive(Clone, Debug)]
pub enum UpdateStatus {
    /// The commit was successfully updated.
    Updated {
        /// An identifier corresponding to the commit, for display purposes only.
        /// This may be a branch name, a change ID, the commit summary, etc.
        ///
        /// This does not necessarily correspond to the commit's name/identifier in
        /// the forge (e.g. not a code review link).
        local_commit_name: String,
    },
}

/// "Forge" refers to a Git hosting provider, such as GitHub, GitLab, etc.
/// Commits can be pushed for review to a forge.
pub trait Forge: Debug {
    /// Get the status of the provided commits.
    fn query_status(
        &mut self,
        commit_set: CommitSet,
    ) -> EyreExitOr<HashMap<NonZeroOid, CommitStatus>>;

    /// Submit the provided set of commits for review.
    fn create(
        &mut self,
        commits: HashMap<NonZeroOid, CommitStatus>,
        options: &SubmitOptions,
    ) -> EyreExitOr<HashMap<NonZeroOid, CreateStatus>>;

    /// Update existing remote commits to match their local versions.
    fn update(
        &mut self,
        commits: HashMap<NonZeroOid, CommitStatus>,
        options: &SubmitOptions,
    ) -> EyreExitOr<HashMap<NonZeroOid, UpdateStatus>>;
}

/// `submit` command.
pub fn command_main(ctx: CommandContext, args: SubmitArgs) -> EyreExitOr<()> {
    let CommandContext {
        effects,
        git_run_info,
    } = ctx;
    let SubmitArgs {
        revsets,
        resolve_revset_options,
        forge_kind,
        create,
        draft,
        message,
        num_jobs,
        execution_strategy,
        dry_run,
    } = args;
    submit(
        &effects,
        &git_run_info,
        revsets,
        &resolve_revset_options,
        forge_kind,
        create,
        draft,
        message,
        num_jobs,
        execution_strategy,
        dry_run,
    )
}

fn submit(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    revsets: Vec<Revset>,
    resolve_revset_options: &ResolveRevsetOptions,
    forge_kind: Option<ForgeKind>,
    create: bool,
    draft: bool,
    message: Option<String>,
    num_jobs: Option<usize>,
    execution_strategy: Option<TestExecutionStrategy>,
    dry_run: bool,
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
        match resolve_commits(effects, &repo, &mut dag, &revsets, resolve_revset_options) {
            Ok(commit_sets) => union_all(&commit_sets),
            Err(err) => {
                err.describe(effects)?;
                return Ok(Err(ExitCode(1)));
            }
        };

    let raw_test_options = RawTestOptions {
        exec: Some("<dummy>".to_string()),
        command: None,
        dry_run: false,
        strategy: execution_strategy,
        search: None,
        bisect: false,
        no_cache: true,
        interactive: false,
        jobs: num_jobs,
        verbosity: Verbosity::None,
        apply_fixes: false,
    };
    let ResolvedTestOptions {
        command: _,
        execution_strategy,
        search_strategy: _,
        is_dry_run: _,
        use_cache: _,
        is_interactive: _,
        num_jobs,
        verbosity: _,
        fix_options: _,
    } = {
        let now = SystemTime::now();
        let event_tx_id =
            event_log_db.make_transaction_id(now, "resolve test options for submit")?;
        try_exit_code!(ResolvedTestOptions::resolve(
            now,
            effects,
            &dag,
            &repo,
            event_tx_id,
            &commit_set,
            None,
            &raw_test_options,
        )?)
    };
    let submit_options = SubmitOptions {
        create,
        draft,
        execution_strategy,
        num_jobs,
        message,
    };

    let unioned_revset = Revset(revsets.iter().map(|Revset(inner)| inner).join(" + "));
    let commit_oids = dag.sort(&commit_set)?;
    let mut forge = select_forge(
        effects,
        git_run_info,
        &repo,
        &mut dag,
        &event_log_db,
        &references_snapshot,
        &unioned_revset,
        forge_kind,
    )?;
    let current_statuses = try_exit_code!(forge.query_status(commit_set)?);
    debug!(?current_statuses, "Commit statuses");

    let create_statuses: HashMap<NonZeroOid, CreateStatus> = {
        let unsubmitted_commits: HashMap<NonZeroOid, CommitStatus> = current_statuses
            .iter()
            .filter(|(_commit_oid, commit_status)| match commit_status {
                CommitStatus {
                    submit_status: SubmitStatus::Unsubmitted,
                    ..
                } => true,
                CommitStatus {
                    submit_status:
                        SubmitStatus::Local
                        | SubmitStatus::NeedsUpdate
                        | SubmitStatus::Unknown
                        | SubmitStatus::UpToDate,
                    ..
                } => false,
            })
            .map(|(commit_oid, create_status)| (*commit_oid, create_status.clone()))
            .collect();
        match (create, dry_run) {
            (false, _) => unsubmitted_commits
                .into_keys()
                .map(|commit_oid| (commit_oid, CreateStatus::Skipped))
                .collect(),

            (true, true) => unsubmitted_commits
                .into_iter()
                .map(|(commit_oid, commit_status)| {
                    (
                        commit_oid,
                        CreateStatus::Created {
                            final_commit_oid: commit_oid,
                            local_commit_name: commit_status
                                .local_commit_name
                                .unwrap_or_else(|| "<unknown>".to_string()),
                        },
                    )
                })
                .collect(),

            (true, false) => try_exit_code!(forge.create(unsubmitted_commits, &submit_options)?),
        }
    };

    let update_statuses: HashMap<NonZeroOid, UpdateStatus> = {
        let commits_to_update: HashMap<NonZeroOid, CommitStatus> = current_statuses
            .iter()
            .filter(|(_commit_oid, commit_status)| match commit_status {
                CommitStatus {
                    submit_status: SubmitStatus::NeedsUpdate,
                    ..
                } => true,
                CommitStatus {
                    submit_status:
                        SubmitStatus::Local
                        | SubmitStatus::Unsubmitted
                        | SubmitStatus::Unknown
                        | SubmitStatus::UpToDate,
                    ..
                } => false,
            })
            .map(|(commit_oid, commit_status)| (*commit_oid, commit_status.clone()))
            .collect();
        if dry_run {
            commits_to_update
                .into_iter()
                .map(|(commit_oid, commit_status)| {
                    (
                        commit_oid,
                        UpdateStatus::Updated {
                            local_commit_name: commit_status
                                .local_commit_name
                                .unwrap_or_else(|| "<unknown>".to_string()),
                        },
                    )
                })
                .collect()
        } else {
            try_exit_code!(forge.update(commits_to_update, &submit_options)?)
        }
    };

    let styled = |style: Style, text: &str| {
        effects
            .get_glyphs()
            .render(StyledString::styled(text, style))
    };
    let mut had_errors = false;
    for commit_oid in commit_oids {
        let current_status = match current_statuses.get(&commit_oid) {
            Some(status) => status,
            None => {
                warn!(?commit_oid, "Could not find commit status");
                continue;
            }
        };

        let commit = repo.find_commit_or_fail(commit_oid)?;
        let commit = effects
            .get_glyphs()
            .render(commit.friendly_describe(effects.get_glyphs())?)?;

        match current_status.submit_status {
            SubmitStatus::Local => continue,
            SubmitStatus::Unknown => {
                warn!(
                    ?commit_oid,
                    "Commit status is unknown; reporting info not implemented."
                );
                continue;
            }
            SubmitStatus::UpToDate => {
                writeln!(
                    effects.get_output_stream(),
                    "{action} {commit} {info}",
                    action = styled(
                        *STYLE_SKIPPED,
                        if dry_run { "Would skip" } else { "Skipped" }
                    )?,
                    info = styled(*STYLE_INFO, "(already up-to-date)")?,
                )?;
            }

            SubmitStatus::Unsubmitted => match create_statuses.get(&commit_oid) {
                Some(CreateStatus::Created {
                    final_commit_oid: _,
                    local_commit_name,
                }) => {
                    writeln!(
                        effects.get_output_stream(),
                        "{action} {commit} {info}",
                        action = styled(
                            *STYLE_SUCCESS,
                            if dry_run { "Would submit" } else { "Submitted" },
                        )?,
                        info = styled(*STYLE_INFO, &format!("(as {local_commit_name})"))?,
                    )?;
                }

                Some(CreateStatus::Skipped) => {
                    writeln!(
                        effects.get_output_stream(),
                        "{action} {commit} {info}",
                        action = styled(
                            *STYLE_SKIPPED,
                            if dry_run { "Would skip" } else { "Skipped" },
                        )?,
                        info = styled(*STYLE_INFO, "(pass --create to submit)")?,
                    )?;
                }

                Some(CreateStatus::Err { message }) => {
                    had_errors = true;
                    writeln!(
                        effects.get_output_stream(),
                        "{action} {commit} {info}",
                        action = styled(
                            *STYLE_FAILED,
                            if dry_run {
                                "Would fail to create"
                            } else {
                                "Failed to create"
                            },
                        )?,
                        info = styled(*STYLE_INFO, &format!("({message})"))?,
                    )?;
                }

                None => {
                    writeln!(
                        effects.get_output_stream(),
                        "{action} {commit} {info}",
                        action = styled(
                            *STYLE_SKIPPED,
                            if dry_run { "Would skip" } else { "Skipped" },
                        )?,
                        info = styled(*STYLE_INFO, "(unknown reason, bug?)")?,
                    )?;
                }
            },

            SubmitStatus::NeedsUpdate => match update_statuses.get(&commit_oid) {
                Some(UpdateStatus::Updated { local_commit_name }) => {
                    writeln!(
                        effects.get_output_stream(),
                        "{action} {commit} {info}",
                        action = styled(
                            *STYLE_SUCCESS,
                            if dry_run { "Would update" } else { "Updated" },
                        )?,
                        info = styled(*STYLE_INFO, &format!("(as {local_commit_name})"))?,
                    )?;
                }

                None => {
                    writeln!(
                        effects.get_output_stream(),
                        "{action} {commit} {info}",
                        action = styled(
                            *STYLE_SKIPPED,
                            if dry_run { "Would skip" } else { "Skipped" },
                        )?,
                        info = styled(*STYLE_INFO, "(unknown reason, bug?)")?,
                    )?;
                }
            },
        };
    }

    if had_errors {
        Ok(Err(ExitCode(1)))
    } else {
        Ok(Ok(()))
    }
}

#[instrument]
fn select_forge<'a>(
    effects: &'a Effects,
    git_run_info: &'a GitRunInfo,
    repo: &'a Repo,
    dag: &'a mut Dag,
    event_log_db: &'a EventLogDb,
    references_snapshot: &'a RepoReferencesSnapshot,
    revset: &'a Revset,
    forge_kind: Option<ForgeKind>,
) -> eyre::Result<Box<dyn Forge + 'a>> {
    // Check if explicitly set:
    let forge_kind = match forge_kind {
        Some(forge_kind) => {
            info!(?forge_kind, "Forge kind was explicitly set");
            Some(forge_kind)
        }
        None => None,
    };

    // Check Phabricator:
    let forge_kind = match forge_kind {
        Some(forge_kind) => Some(forge_kind),
        None => {
            let use_phabricator = if let Some(working_copy_path) = repo.get_working_copy_path() {
                let arcconfig_path = &working_copy_path.join(".arcconfig");
                let arcconfig_present = arcconfig_path.is_file();
                debug!(
                    ?arcconfig_path,
                    ?arcconfig_present,
                    "Checking arcconfig path to decide whether to use Phabricator"
                );
                arcconfig_present
            } else {
                false
            };
            use_phabricator.then_some(ForgeKind::Phabricator)
        }
    };

    // Check Github:
    let is_github_forge_reliable_enough_for_opt_out_usage = false; // as of 2024-04-06 it's too buggy; see https://github.com/arxanas/git-branchless/discussions/1259
    let forge_kind = match (
        forge_kind,
        is_github_forge_reliable_enough_for_opt_out_usage,
    ) {
        (Some(forge_kind), _) => Some(forge_kind),
        (None, true) => github_push_remote(repo)?.map(|_| ForgeKind::Github),
        (None, false) => None,
    };

    // Default:
    let forge_kind = forge_kind.unwrap_or(ForgeKind::Branch);

    info!(?forge_kind, "Selected forge kind");
    let forge: Box<dyn Forge> = match forge_kind {
        ForgeKind::Branch => Box::new(BranchForge {
            effects,
            git_run_info,
            repo,
            dag,
            event_log_db,
            references_snapshot,
        }),

        ForgeKind::Github => Box::new(GithubForge {
            effects,
            git_run_info,
            repo,
            dag,
            event_log_db,
            client: GithubForge::client(git_run_info.clone()),
        }),

        ForgeKind::Phabricator => Box::new(PhabricatorForge {
            effects,
            git_run_info,
            repo,
            dag,
            event_log_db,
            revset,
        }),
    };
    Ok(forge)
}
