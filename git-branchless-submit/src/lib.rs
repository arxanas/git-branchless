//! Push the user's commits to a remote.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

mod branch_push_forge;

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

use branch_push_forge::BranchPushForge;
use cursive_core::theme::{BaseColor, Effect, Style};
use git_branchless_invoke::CommandContext;
use itertools::Itertools;
use lazy_static::lazy_static;
use lib::core::dag::Dag;
use lib::core::effects::Effects;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::{Pluralize, StyledStringBuilder};
use lib::core::repo_ext::RepoExt;
use lib::git::{GitRunInfo, NonZeroOid, Repo};
use lib::util::ExitCode;

use git_branchless_opts::{ResolveRevsetOptions, Revset, SubmitArgs};
use git_branchless_revset::resolve_commits;

lazy_static! {
    static ref STYLE_PUSHED: Style =
        Style::merge(&[BaseColor::Green.light().into(), Effect::Bold.into()]);
    static ref STYLE_SKIPPED: Style =
        Style::merge(&[BaseColor::Yellow.light().into(), Effect::Bold.into()]);
}

/// The status of a commit, indicating whether it needs to be updated remotely.
#[derive(Clone, Debug)]
pub enum SubmitStatus {
    /// The commit exists locally but has not been pushed remotely.
    Unsubmitted,

    /// It could not be determined whether the remote commit exists.
    Unresolved,

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
pub struct SubmitOptions {
    /// Create associated branches, code reviews, etc. for each of the provided commits.
    ///
    /// This should be an idempotent behavior, i.e. setting `create` to `true`
    /// and submitting a commit which already has an associated remote item
    /// should not have any additional effect.
    pub create: bool,
}

/// `submit` command.
pub fn command_main(ctx: CommandContext, args: SubmitArgs) -> eyre::Result<ExitCode> {
    let CommandContext {
        effects,
        git_run_info,
    } = ctx;
    let SubmitArgs {
        create,
        revset,
        resolve_revset_options,
    } = args;
    submit(
        &effects,
        &git_run_info,
        revset,
        &resolve_revset_options,
        create,
    )
}

fn submit(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    revset: Revset,
    resolve_revset_options: &ResolveRevsetOptions,
    create: bool,
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

    let submit_options = SubmitOptions { create };
    let forge = BranchPushForge {
        effects,
        git_run_info,
        repo: &repo,
        dag: &dag,
        event_log_db: &event_log_db,
        references_snapshot: &references_snapshot,
    };
    let statuses = match forge.query_status(commit_set)? {
        Ok(statuses) => statuses,
        Err(exit_code) => return Ok(exit_code),
    };

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
                local_branch_name: Some(_),
                remote_branch_name: _,
            } => {
                unsubmitted.insert(commit_oid, commit_status);
            }

            // Not currently implemented: generating a branch for
            // unsubmitted commits which don't yet have branches.
            CommitStatus {
                submit_status: SubmitStatus::Unsubmitted,
                remote_name: _,
                local_branch_name: None,
                remote_branch_name: _,
            } => {}

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
                submit_status: SubmitStatus::Unresolved,
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
            let exit_code = forge.create(unsubmitted_commits, &submit_options)?;
            if !exit_code.is_success() {
                return Ok(exit_code);
            }
            (unsubmitted_branches, Default::default())
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

        let exit_code = forge.update(commits_to_update, &submit_options)?;
        if !exit_code.is_success() {
            return Ok(exit_code);
        }
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

    Ok(ExitCode(0))
}
