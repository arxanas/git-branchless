//! Amend the current commit.
//!
//! This command amends the HEAD commit with changes to files
//! that are already tracked in the repo. Following the amend,
//! the command performs a restack.

use std::convert::TryFrom;
use std::ffi::OsString;
use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use eden_dag::DagAlgorithm;
use eyre::Context;
use git_branchless_opts::{MoveOptions, ResolveRevsetOptions};
use itertools::Itertools;
use lib::core::check_out::{check_out_commit, CheckOutCommitOptions, CheckoutTarget};
use lib::core::config::get_restack_preserve_timestamps;
use lib::core::dag::commit_set_to_vec;
use lib::core::dag::{CommitSet, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{Event, EventLogDb, EventReplayer};
use lib::core::formatting::Pluralize;
use lib::core::gc::mark_commit_reachable;
use lib::core::repo_ext::RepoExt;
use lib::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanOptions, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    RebasePlanBuilder, RebasePlanPermissions, RepoResource,
};
use lib::git::{
    AmendFastOptions, CategorizedReferenceName, GitRunInfo, MaybeZeroOid, NonZeroOid, Repo,
    ResolvedReferenceInfo,
};
use lib::util::ExitCode;
use rayon::ThreadPoolBuilder;
use tracing::instrument;

/// Amends the existing HEAD commit.
#[instrument]
pub fn amend(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    resolve_revset_options: &ResolveRevsetOptions,
    move_options: &MoveOptions,
    reparent: bool,
) -> eyre::Result<ExitCode> {
    let now = SystemTime::now();
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();
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

    let head_info = repo.get_head_info()?;
    let head_oid = match head_info.oid {
        Some(oid) => oid,
        None => {
            writeln!(
                effects.get_output_stream(),
                "No commit is currently checked out. Check out a commit to amend and then try again.",
            )?;
            return Ok(ExitCode(1));
        }
    };
    let head_commit = repo.find_commit_or_fail(head_oid)?;

    let index = repo.get_index()?;
    if index.has_conflicts() {
        writeln!(
            effects.get_output_stream(),
            "Cannot amend, because there are unresolved merge conflicts. Resolve the merge conflicts and try again."
        )?;
        return Ok(ExitCode(1));
    }

    let build_options = BuildRebasePlanOptions {
        force_rewrite_public_commits: move_options.force_rewrite_public_commits,
        dump_rebase_constraints: move_options.dump_rebase_constraints,
        dump_rebase_plan: move_options.dump_rebase_plan,
        detect_duplicate_commits_via_patch_id: move_options.detect_duplicate_commits_via_patch_id,
    };
    let commits_to_verify = dag.query().descendants(CommitSet::from(head_oid))?;
    let commits_to_verify = dag.filter_visible_commits(commits_to_verify)?;
    if let Err(err) =
        RebasePlanPermissions::verify_rewrite_set(&dag, &build_options, &commits_to_verify)?
    {
        err.describe(effects, &repo)?;
        return Ok(ExitCode(1));
    };

    let event_tx_id = event_log_db.make_transaction_id(now, "amend")?;
    let (snapshot, status) =
        repo.get_status(effects, git_run_info, &index, &head_info, Some(event_tx_id))?;
    {
        let ResolvedReferenceInfo {
            oid,
            reference_name,
        } = &head_info;
        event_log_db.add_events(vec![Event::WorkingCopySnapshot {
            timestamp,
            event_tx_id,
            head_oid: MaybeZeroOid::from(*oid),
            commit_oid: snapshot.base_commit.get_oid(),
            ref_name: reference_name.clone(),
        }])?;
    }

    // Note that there may be paths which are in both of these entries in the
    // case that the given path has both staged and unstaged changes.
    let staged_entries = status
        .clone()
        .into_iter()
        .filter(|entry| entry.index_status.is_changed())
        .collect_vec();
    let unstaged_entries = status
        .into_iter()
        .filter(|entry| entry.working_copy_status.is_changed())
        .collect_vec();

    let opts = if !staged_entries.is_empty() {
        AmendFastOptions::FromIndex {
            paths: staged_entries
                .into_iter()
                .flat_map(|entry| entry.paths())
                .collect(),
        }
    } else {
        AmendFastOptions::FromWorkingCopy {
            status_entries: unstaged_entries.clone(),
        }
    };
    if opts.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "There are no uncommitted or staged changes. Nothing to amend."
        )?;
        return Ok(ExitCode(0));
    }

    let amended_tree = repo.amend_fast(&head_commit, &opts)?;

    let (author, committer) = (head_commit.get_author(), head_commit.get_committer());
    let (author, committer) = if get_restack_preserve_timestamps(&repo)? {
        (author, committer)
    } else {
        (
            author.update_timestamp(now)?,
            committer.update_timestamp(now)?,
        )
    };

    let amended_commit_oid = head_commit.amend_commit(
        None,
        Some(&author),
        Some(&committer),
        None,
        Some(&amended_tree),
    )?;
    mark_commit_reachable(&repo, amended_commit_oid)
        .wrap_err("Marking commit as reachable for GC purposes.")?;
    dag.sync_from_oids(
        effects,
        &repo,
        CommitSet::empty(),
        CommitSet::from(amended_commit_oid),
    )?;
    let exit_code = {
        let additional_args = match &head_info.reference_name {
            Some(name) => match CategorizedReferenceName::new(name) {
                name @ CategorizedReferenceName::LocalBranch { .. } => {
                    vec![OsString::from("-B"), OsString::from(name.remove_prefix()?)]
                }
                CategorizedReferenceName::RemoteBranch { .. }
                | CategorizedReferenceName::OtherRef { .. } => Default::default(),
            },
            None => Default::default(),
        };
        check_out_commit(
            effects,
            git_run_info,
            &repo,
            &event_log_db,
            event_tx_id,
            Some(CheckoutTarget::Oid(amended_commit_oid)),
            &CheckOutCommitOptions {
                additional_args,
                reset: true,
                render_smartlog: false,
            },
        )?
    };
    if !exit_code.is_success() {
        return Ok(exit_code);
    }

    let rebase_plan = {
        let build_options = BuildRebasePlanOptions {
            force_rewrite_public_commits: move_options.force_rewrite_public_commits,
            detect_duplicate_commits_via_patch_id: move_options
                .detect_duplicate_commits_via_patch_id,
            dump_rebase_constraints: move_options.dump_rebase_constraints,
            dump_rebase_plan: move_options.dump_rebase_plan,
        };
        let commits_to_verify = dag.query().descendants(CommitSet::from(head_oid))?;
        let commits_to_verify = dag.filter_visible_commits(commits_to_verify)?;
        let permissions = match RebasePlanPermissions::verify_rewrite_set(
            &dag,
            &build_options,
            &commits_to_verify,
        )? {
            Ok(permissions) => permissions,
            Err(err) => {
                err.describe(effects, &repo)?;
                return Ok(ExitCode(1));
            }
        };

        let mut builder = RebasePlanBuilder::new(&dag, permissions);
        builder.move_subtree(head_oid, head_commit.get_parent_oids())?;
        builder.replace_commit(head_oid, amended_commit_oid)?;

        // To keep the contents of all descendant commits the same, forcibly
        // replace the children commits, and then rely on normal patch
        // application to apply the rest.
        if reparent {
            let descendants = dag
                .query()
                .descendants(CommitSet::from(head_oid))?
                .difference(&CommitSet::from(head_oid));
            let descendants = dag.filter_visible_commits(descendants)?;
            for descendant_oid in commit_set_to_vec(&descendants)? {
                let parents = dag.query().parent_names(descendant_oid.into())?;
                builder.move_subtree(
                    descendant_oid,
                    parents
                        .into_iter()
                        .map(NonZeroOid::try_from)
                        .try_collect()?,
                )?;
                builder.replace_commit(descendant_oid, descendant_oid)?;
            }
        }

        let thread_pool = ThreadPoolBuilder::new().build()?;
        let repo_pool = RepoResource::new_pool(&repo)?;
        match builder.build(effects, &thread_pool, &repo_pool)? {
            Ok(Some(rebase_plan)) => rebase_plan,
            Ok(None) => {
                unreachable!("A rebase plan should always be generated when amending a commit.");
            }
            Err(err) => {
                err.describe(effects, &repo)?;
                return Ok(ExitCode(1));
            }
        }
    };

    let execute_options = ExecuteRebasePlanOptions {
        now,
        event_tx_id,
        force_in_memory: move_options.force_in_memory,
        force_on_disk: move_options.force_on_disk,
        preserve_timestamps: get_restack_preserve_timestamps(&repo)?,
        resolve_merge_conflicts: move_options.resolve_merge_conflicts,
        check_out_commit_options: CheckOutCommitOptions {
            additional_args: Default::default(),
            reset: true,
            render_smartlog: true,
        },
    };
    let needs_merge = match execute_rebase_plan(
        effects,
        git_run_info,
        &repo,
        &event_log_db,
        &rebase_plan,
        &execute_options,
    )? {
        ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ } => false,
        ExecuteRebasePlanResult::DeclinedToMerge {
            merge_conflict: _, // `execute_rebase_plan` should have printed merge conflict info.
        } => {
            let checkout_args = match head_info {
                ResolvedReferenceInfo {
                    oid: _,
                    reference_name: Some(reference_name),
                } => vec![
                    OsString::from("-B"),
                    OsString::from(reference_name.as_str()),
                ],
                ResolvedReferenceInfo {
                    oid: _,
                    reference_name: None,
                } => Vec::new(),
            };

            writeln!(
                effects.get_output_stream(),
                "This operation would cause a merge conflict, and --merge was not provided."
            )?;
            writeln!(
                effects.get_output_stream(),
                "Amending without rebasing descendants: {}",
                effects
                    .get_glyphs()
                    .render(head_commit.friendly_describe(effects.get_glyphs())?)?
            )?;

            event_log_db.add_events(vec![Event::RewriteEvent {
                timestamp: now.duration_since(UNIX_EPOCH)?.as_secs_f64(),
                event_tx_id,
                old_commit_oid: MaybeZeroOid::NonZero(head_oid),
                new_commit_oid: MaybeZeroOid::NonZero(amended_commit_oid),
            }])?;
            let exit_code = check_out_commit(
                effects,
                git_run_info,
                &repo,
                &event_log_db,
                event_tx_id,
                Some(CheckoutTarget::Oid(amended_commit_oid)),
                &CheckOutCommitOptions {
                    additional_args: checkout_args,
                    reset: true,
                    render_smartlog: true,
                },
            )?;
            if !exit_code.is_success() {
                return Ok(exit_code);
            }

            true
        }
        ExecuteRebasePlanResult::Failed { exit_code } => {
            return Ok(exit_code);
        }
    };

    match opts {
        AmendFastOptions::FromIndex { paths } => {
            let staged_changes = Pluralize {
                determiner: None,
                amount: paths.len(),
                unit: ("staged change", "staged changes"),
            };
            let mut message = format!("Amended with {staged_changes}.");
            // TODO: Include the number of uncommitted changes.
            if !unstaged_entries.is_empty() {
                message += " (Some uncommitted changes were not amended.)";
            }
            writeln!(effects.get_output_stream(), "{message}")?;
        }
        AmendFastOptions::FromWorkingCopy { status_entries } => {
            let uncommitted_changes = Pluralize {
                determiner: None,
                amount: status_entries.len(),
                unit: ("uncommitted change", "uncommitted changes"),
            };
            writeln!(
                effects.get_output_stream(),
                "Amended with {uncommitted_changes}.",
            )?;
        }
    }

    if needs_merge {
        writeln!(
            effects.get_output_stream(),
            "To resolve merge conflicts run: git restack --merge"
        )?;
    }

    Ok(ExitCode(0))
}
