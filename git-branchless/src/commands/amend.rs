//! Amend the current commit.
//!
//! This command amends the HEAD commit with changes to files
//! that are already tracked in the repo. Following the amend,
//! the command performs a restack.

use std::collections::HashMap;

use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use bstr::ByteSlice;

use eyre::Context;
use git_branchless_opts::{MoveOptions, ResolveRevsetOptions};
use itertools::Itertools;
use lib::core::check_out::{check_out_commit, CheckOutCommitOptions, CheckoutTarget};
use lib::core::config::get_restack_preserve_timestamps;
use lib::core::dag::{CommitSet, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{Event, EventLogDb, EventReplayer};
use lib::core::formatting::Pluralize;
use lib::core::gc::mark_commit_reachable;
use lib::core::repo_ext::RepoExt;
use lib::core::rewrite::{
    execute_rebase_plan, move_branches, BuildRebasePlanOptions, ExecuteRebasePlanOptions,
    ExecuteRebasePlanResult, RebasePlanBuilder, RebasePlanPermissions, RepoResource,
};
use lib::git::get_signer;
use lib::git::{AmendFastOptions, GitRunInfo, MaybeZeroOid, Repo, ResolvedReferenceInfo};
use lib::try_exit_code;
use lib::util::{ExitCode, EyreExitOr};
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
) -> EyreExitOr<()> {
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
            return Ok(Err(ExitCode(1)));
        }
    };
    let head_commit = repo.find_commit_or_fail(head_oid)?;

    let index = repo.get_index()?;
    if index.has_conflicts() {
        writeln!(
            effects.get_output_stream(),
            "Cannot amend, because there are unresolved merge conflicts. Resolve the merge conflicts and try again."
        )?;
        return Ok(Err(ExitCode(1)));
    }

    let build_options = BuildRebasePlanOptions {
        force_rewrite_public_commits: move_options.force_rewrite_public_commits,
        dump_rebase_constraints: move_options.dump_rebase_constraints,
        dump_rebase_plan: move_options.dump_rebase_plan,
        detect_duplicate_commits_via_patch_id: move_options.detect_duplicate_commits_via_patch_id,
    };
    let commits_to_verify = dag.query_descendants(CommitSet::from(head_oid))?;
    let commits_to_verify = dag.filter_visible_commits(commits_to_verify)?;
    if let Err(err) =
        RebasePlanPermissions::verify_rewrite_set(&dag, build_options, &commits_to_verify)?
    {
        err.describe(effects, &repo, &dag)?;
        return Ok(Err(ExitCode(1)));
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
        return Ok(Ok(()));
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

    let sign_option = move_options.sign_options.to_owned().into();
    let signer = get_signer(&repo, &sign_option)?;

    let amended_commit_oid = repo.amend_commit(
        &head_commit,
        Some(&author),
        Some(&committer),
        None,
        Some(&amended_tree),
        signer.as_deref(),
    )?;

    // Switch to the new commit and move any branches. This is kind of a hack:
    // ideally, we would use the same rebase plan machinery to accomplish this
    // and also rebase any descendants. However, this operation should always
    // succeed, and we want to execute it regardless of whether the rest of the
    // rebase would succeed without conflicts, so instead we (re)write a bunch
    // of logic to switch commits and move branches.
    {
        mark_commit_reachable(&repo, amended_commit_oid)
            .wrap_err("Marking commit as reachable for GC purposes.")?;
        event_log_db.add_events(vec![Event::RewriteEvent {
            timestamp: now.duration_since(UNIX_EPOCH)?.as_secs_f64(),
            event_tx_id,
            old_commit_oid: MaybeZeroOid::NonZero(head_oid),
            new_commit_oid: MaybeZeroOid::NonZero(amended_commit_oid),
        }])?;
        dag.sync_from_oids(
            effects,
            &repo,
            CommitSet::empty(),
            CommitSet::from(amended_commit_oid),
        )?;
        move_branches(effects, git_run_info, &repo, event_tx_id, &{
            let mut result = HashMap::new();
            result.insert(head_oid, MaybeZeroOid::NonZero(amended_commit_oid));
            result
        })?;

        let target = match &head_info.reference_name {
            Some(name) => CheckoutTarget::Reference(name.clone()),
            None => CheckoutTarget::Oid(amended_commit_oid),
        };
        try_exit_code!(check_out_commit(
            effects,
            git_run_info,
            &repo,
            &event_log_db,
            event_tx_id,
            Some(target),
            &CheckOutCommitOptions {
                additional_args: Default::default(),
                reset: true,
                render_smartlog: false,
            },
        )?);
    }

    let rebase_plan = {
        let build_options = BuildRebasePlanOptions {
            force_rewrite_public_commits: move_options.force_rewrite_public_commits,
            detect_duplicate_commits_via_patch_id: move_options
                .detect_duplicate_commits_via_patch_id,
            dump_rebase_constraints: move_options.dump_rebase_constraints,
            dump_rebase_plan: move_options.dump_rebase_plan,
        };
        let children = dag.query_children(CommitSet::from(head_oid))?;
        let descendants = dag.query_descendants(children)?;
        let descendants = dag.filter_visible_commits(descendants)?;
        let commits_to_verify = &descendants;
        let permissions = match RebasePlanPermissions::verify_rewrite_set(
            &dag,
            build_options,
            commits_to_verify,
        )? {
            Ok(permissions) => permissions,
            Err(err) => {
                err.describe(effects, &repo, &dag)?;
                return Ok(Err(ExitCode(1)));
            }
        };

        let mut builder = RebasePlanBuilder::new(&dag, permissions);
        for descendant_oid in dag.commit_set_to_vec(&descendants)? {
            let descendant_commit = repo.find_commit_or_fail(descendant_oid)?;
            let parent_oids: Vec<_> = descendant_commit
                .get_parent_oids()
                .into_iter()
                .map(|parent_oid| {
                    if parent_oid == head_oid {
                        amended_commit_oid
                    } else {
                        parent_oid
                    }
                })
                .collect();
            builder.move_subtree(descendant_oid, parent_oids.clone())?;

            // To keep the contents of all descendant commits the same, forcibly
            // replace the children commits, and then rely on normal patch
            // application to apply the rest.
            if reparent {
                let parents: Vec<_> = parent_oids
                    .into_iter()
                    .map(|parent_oid| repo.find_commit_or_fail(parent_oid))
                    .try_collect()?;
                let descendant_message = descendant_commit.get_message_raw();
                let descendant_message = descendant_message.to_str().with_context(|| {
                    eyre::eyre!(
                        "Could not decode commit message for descendant commit: {:?}",
                        descendant_commit
                    )
                })?;
                let reparented_descendant_oid = repo.create_commit(
                    &descendant_commit.get_author(),
                    &descendant_commit.get_committer(),
                    descendant_message,
                    &descendant_commit.get_tree()?,
                    parents.iter().collect(),
                    signer.as_deref(),
                )?;
                builder.replace_commit(descendant_oid, reparented_descendant_oid)?;
            }
        }

        let thread_pool = ThreadPoolBuilder::new().build()?;
        let repo_pool = RepoResource::new_pool(&repo)?;
        match builder.build(effects, &thread_pool, &repo_pool)? {
            Ok(rebase_plan) => rebase_plan,
            Err(err) => {
                err.describe(effects, &repo, &dag)?;
                return Ok(Err(ExitCode(1)));
            }
        }
    };

    if let Some(rebase_plan) = rebase_plan {
        let execute_options = ExecuteRebasePlanOptions {
            now,
            event_tx_id,
            force_in_memory: move_options.force_in_memory,
            force_on_disk: move_options.force_on_disk,
            preserve_timestamps: get_restack_preserve_timestamps(&repo)?,
            resolve_merge_conflicts: move_options.resolve_merge_conflicts,
            check_out_commit_options: CheckOutCommitOptions {
                additional_args: Default::default(),
                reset: false,
                render_smartlog: false,
            },
            sign_option,
        };
        match execute_rebase_plan(
            effects,
            git_run_info,
            &repo,
            &event_log_db,
            &rebase_plan,
            &execute_options,
        )? {
            ExecuteRebasePlanResult::Succeeded {
                rewritten_oids: None,
            } => {}

            ExecuteRebasePlanResult::Succeeded {
                rewritten_oids: Some(rewritten_oids),
            } => {
                writeln!(
                    effects.get_output_stream(),
                    "Restacked {}.",
                    Pluralize {
                        determiner: None,
                        amount: rewritten_oids.len(),
                        unit: ("commit", "commits")
                    }
                )?;
            }

            ExecuteRebasePlanResult::DeclinedToMerge { failed_merge_info } => {
                failed_merge_info.describe(
                    effects,
                    &repo,
                    lib::core::rewrite::MergeConflictRemediation::Restack,
                )?;
                writeln!(
                    effects.get_output_stream(),
                    "Amending without restacking descendant commits: {}",
                    effects
                        .get_glyphs()
                        .render(head_commit.friendly_describe(effects.get_glyphs())?)?
                )?;
            }

            ExecuteRebasePlanResult::Failed { exit_code } => {
                return Ok(Err(exit_code));
            }
        }
    }

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
        AmendFastOptions::FromCommit { .. } => {
            unreachable!("BUG: AmendFastOptions::FromCommit should not have been constructed.")
        }
    }

    Ok(Ok(()))
}
