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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::{Debug, Write};
use std::time::SystemTime;

use branch_forge::BranchForge;
use cursive_core::theme::{BaseColor, Effect, Style};
use git_branchless_invoke::CommandContext;
use git_branchless_test::{RawTestOptions, ResolvedTestOptions, Verbosity};
use github::GithubForge;
use itertools::Itertools;
use lazy_static::lazy_static;
use lib::core::dag::{CommitSet, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::{Pluralize, StyledStringBuilder};
use lib::core::repo_ext::{RepoExt, RepoReferencesSnapshot};
use lib::git::{GitRunInfo, NonZeroOid, ReferenceName, Repo};
use lib::try_exit_code;
use lib::util::{ExitCode, EyreExitOr};

use git_branchless_opts::{
    ForgeKind, ResolveRevsetOptions, Revset, SubmitArgs, TestExecutionStrategy,
};
use git_branchless_revset::resolve_commits;
use phabricator::PhabricatorForge;
use tracing::{debug, info, instrument, warn};

lazy_static! {
    /// The style for branches which were successfully submitted.
    pub static ref STYLE_PUSHED: Style =
        Style::merge(&[BaseColor::Green.light().into(), Effect::Bold.into()]);

    /// The style for branches which were not submitted.
    pub static ref STYLE_SKIPPED: Style =
        Style::merge(&[BaseColor::Yellow.light().into(), Effect::Bold.into()]);
}

/// The status of a commit, indicating whether it needs to be updated remotely.
#[derive(Clone, Debug)]
pub enum SubmitStatus {
    /// The commit exists locally but has not been pushed remotely.
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
    submit_status: SubmitStatus,
    remote_name: Option<String>,
    local_branch_name: Option<String>,
    #[allow(dead_code)]
    remote_branch_name: Option<String>,
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
pub struct CreateStatus {
    /// The commit OID after carrying out the creation process. Usually, this
    /// will be the same as the original commit OID, unless the forge amends it
    /// (e.g. to include a change ID).
    pub final_commit_oid: NonZeroOid,

    /// The local branch name to use. The caller will try to create the branch
    /// pointing to that commit (assuming that it doesn't already exist).
    pub local_branch_name: String,
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
    ) -> EyreExitOr<()>;
}

/// `submit` command.
pub fn command_main(ctx: CommandContext, args: SubmitArgs) -> EyreExitOr<()> {
    let CommandContext {
        effects,
        git_run_info,
    } = ctx;
    let SubmitArgs {
        create,
        draft,
        strategy,
        revset,
        resolve_revset_options,
        forge,
        message,
    } = args;
    submit(
        &effects,
        &git_run_info,
        revset,
        &resolve_revset_options,
        create,
        draft,
        strategy,
        forge,
        message,
    )
}

fn submit(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    revset: Revset,
    resolve_revset_options: &ResolveRevsetOptions,
    create: bool,
    draft: bool,
    execution_strategy: Option<TestExecutionStrategy>,
    forge_kind: Option<ForgeKind>,
    message: Option<String>,
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
        jobs: None,
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

    let mut forge = select_forge(
        effects,
        git_run_info,
        &repo,
        &mut dag,
        &event_log_db,
        &references_snapshot,
        &revset,
        forge_kind,
    );
    let statuses = try_exit_code!(forge.query_status(commit_set)?);
    debug!(?statuses, "Commit statuses");

    let (unsubmitted_commits, commits_to_update, commits_to_skip): (
        HashMap<NonZeroOid, CommitStatus>,
        HashMap<NonZeroOid, CommitStatus>,
        HashMap<NonZeroOid, CommitStatus>,
    ) = statuses.into_iter().fold(Default::default(), |acc, elem| {
        let (mut unsubmitted, mut to_update, mut to_skip) = acc;
        let (commit_oid, commit_status) = elem;
        match commit_status {
            CommitStatus {
                submit_status: SubmitStatus::Unsubmitted,
                remote_name: _,
                local_branch_name: _,
                remote_branch_name: _,
            } => {
                unsubmitted.insert(commit_oid, commit_status);
            }

            CommitStatus {
                submit_status: SubmitStatus::NeedsUpdate,
                remote_name: _,
                local_branch_name: _,
                remote_branch_name: _,
            } => {
                to_update.insert(commit_oid, commit_status);
            }

            CommitStatus {
                submit_status: SubmitStatus::UpToDate,
                remote_name: _,
                local_branch_name: Some(_),
                remote_branch_name: _,
            } => {
                to_skip.insert(commit_oid, commit_status);
            }

            // Don't know what to do in these cases 🙃.
            CommitStatus {
                submit_status: SubmitStatus::Unknown,
                remote_name: _,
                local_branch_name: _,
                remote_branch_name: _,
            }
            | CommitStatus {
                submit_status: SubmitStatus::UpToDate,
                remote_name: _,
                local_branch_name: None,
                remote_branch_name: _,
            } => {}
        }
        (unsubmitted, to_update, to_skip)
    });

    let (created_branches, uncreated_branches): (BTreeSet<String>, BTreeSet<String>) = {
        let unsubmitted_branches = unsubmitted_commits
            .values()
            .flat_map(|commit_status| commit_status.local_branch_name.clone())
            .collect();
        if unsubmitted_commits.is_empty() {
            Default::default()
        } else if create {
            let create_statuses =
                try_exit_code!(forge.create(unsubmitted_commits, &submit_options)?);
            let all_branches: HashSet<_> = references_snapshot
                .branch_oid_to_names
                .values()
                .flatten()
                .collect();
            let mut created_branches = BTreeSet::new();
            for (_commit_oid, create_status) in create_statuses {
                let CreateStatus {
                    final_commit_oid,
                    local_branch_name,
                } = create_status;
                let branch_reference_name =
                    ReferenceName::from(format!("refs/heads/{local_branch_name}"));
                created_branches.insert(local_branch_name);
                if !all_branches.contains(&branch_reference_name) {
                    repo.create_reference(
                        &branch_reference_name,
                        final_commit_oid,
                        false,
                        "submit",
                    )?;
                }
            }
            (created_branches, Default::default())
        } else {
            (Default::default(), unsubmitted_branches)
        }
    };

    let (updated_branch_names, skipped_branch_names): (BTreeSet<String>, BTreeSet<String>) = {
        let updated_branch_names = commits_to_update
            .iter()
            .flat_map(|(_commit_oid, commit_status)| commit_status.local_branch_name.clone())
            .collect();
        let skipped_branch_names = commits_to_skip
            .iter()
            .flat_map(|(_commit_oid, commit_status)| commit_status.local_branch_name.clone())
            .collect();

        try_exit_code!(forge.update(commits_to_update, &submit_options)?);
        (updated_branch_names, skipped_branch_names)
    };

    if !created_branches.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "Created {}: {}",
            Pluralize {
                determiner: None,
                amount: created_branches.len(),
                unit: ("branch", "branches")
            },
            created_branches
                .into_iter()
                .map(|branch_name| effects
                    .get_glyphs()
                    .render(
                        StyledStringBuilder::new()
                            .append_styled(branch_name, *STYLE_PUSHED)
                            .build(),
                    )
                    .expect("Rendering branch name"))
                .join(", ")
        )?;
    }
    if !updated_branch_names.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "Pushed {}: {}",
            Pluralize {
                determiner: None,
                amount: updated_branch_names.len(),
                unit: ("branch", "branches")
            },
            updated_branch_names
                .into_iter()
                .map(|branch_name| effects
                    .get_glyphs()
                    .render(
                        StyledStringBuilder::new()
                            .append_styled(branch_name, *STYLE_PUSHED)
                            .build(),
                    )
                    .expect("Rendering branch name"))
                .join(", ")
        )?;
    }
    if !skipped_branch_names.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "Skipped {} (already up-to-date): {}",
            Pluralize {
                determiner: None,
                amount: skipped_branch_names.len(),
                unit: ("branch", "branches")
            },
            skipped_branch_names
                .into_iter()
                .map(|branch_name| effects
                    .get_glyphs()
                    .render(
                        StyledStringBuilder::new()
                            .append_styled(branch_name, *STYLE_SKIPPED)
                            .build(),
                    )
                    .expect("Rendering branch name"))
                .join(", ")
        )?;
    }
    if !uncreated_branches.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "Skipped {} (not yet on remote): {}",
            Pluralize {
                determiner: None,
                amount: uncreated_branches.len(),
                unit: ("branch", "branches")
            },
            uncreated_branches
                .into_iter()
                .map(|branch_name| effects
                    .get_glyphs()
                    .render(
                        StyledStringBuilder::new()
                            .append_styled(branch_name, *STYLE_SKIPPED)
                            .build(),
                    )
                    .expect("Rendering branch name"))
                .join(", ")
        )?;
        writeln!(
            effects.get_output_stream(),
            "\
These branches were skipped because they were not already associated with a remote repository. To
create and push them, retry this operation with the --create option."
        )?;
    }

    Ok(Ok(()))
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
    forge: Option<ForgeKind>,
) -> Box<dyn Forge + 'a> {
    let forge_kind = match forge {
        Some(forge_kind) => {
            info!(?forge_kind, "Forge kind was explicitly set");
            forge_kind
        }
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
            if use_phabricator {
                ForgeKind::Phabricator
            } else {
                ForgeKind::Branch
            }
        }
    };

    info!(?forge_kind, "Selected forge kind");
    match forge_kind {
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
        }),

        ForgeKind::Phabricator => Box::new(PhabricatorForge {
            effects,
            git_run_info,
            repo,
            dag,
            event_log_db,
            revset,
        }),
    }
}
