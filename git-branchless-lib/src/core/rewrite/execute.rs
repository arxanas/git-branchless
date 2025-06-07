use std::collections::{HashMap, HashSet};

use std::fmt::Write;
use std::path::PathBuf;
use std::time::SystemTime;

use bstr::BString;
use eyre::Context;
use tracing::warn;

use crate::core::check_out::{check_out_commit, CheckOutCommitOptions, CheckoutTarget};
use crate::core::effects::Effects;
use crate::core::eventlog::{EventLogDb, EventTransactionId};
use crate::core::formatting::Pluralize;
use crate::core::repo_ext::RepoExt;
use crate::git::{
    BranchType, CategorizedReferenceName, GitRunInfo, MaybeZeroOid, NonZeroOid, ReferenceName,
    Repo, ResolvedReferenceInfo, SignOption,
};
use crate::util::{ExitCode, EyreExitOr};

use super::plan::RebasePlan;

/// Given a list of rewritten OIDs, move the branches attached to those OIDs
/// from their old commits to their new commits. Invoke the
/// `reference-transaction` hook when done.
pub fn move_branches<'a>(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &'a Repo,
    event_tx_id: EventTransactionId,
    rewritten_oids_map: &'a HashMap<NonZeroOid, MaybeZeroOid>,
) -> eyre::Result<()> {
    let main_branch = repo.get_main_branch()?;
    let main_branch_name = main_branch.get_reference_name()?;
    let branch_oid_to_names = repo.get_branch_oid_to_names()?;

    // We may experience an error in the case of a branch move. Ideally, we
    // would use `git2::Transaction::commit`, which stops the transaction at the
    // first error, but we don't know which references we successfully committed
    // in that case. Instead, we just do things non-atomically and record which
    // ones succeeded. See https://github.com/libgit2/libgit2/issues/5918
    let mut branch_moves: Vec<(NonZeroOid, MaybeZeroOid, &ReferenceName)> = Vec::new();
    let mut branch_move_err: Option<eyre::Error> = None;
    'outer: for (old_oid, names) in branch_oid_to_names.iter() {
        let new_oid = match rewritten_oids_map.get(old_oid) {
            Some(new_oid) => new_oid,
            None => continue,
        };
        let mut names: Vec<_> = names.iter().collect();
        // Sort for determinism in tests.
        names.sort_unstable();
        match new_oid {
            MaybeZeroOid::NonZero(new_oid) => {
                let new_commit = match repo.find_commit_or_fail(*new_oid).wrap_err_with(|| {
                    format!(
                        "Could not find newly-rewritten commit with old OID: {old_oid:?}, new OID: {new_oid:?}",
                    )
                }) {
                    Ok(commit) => commit,
                    Err(err) => {
                        branch_move_err = Some(err);
                        break 'outer;
                    }
                };

                for reference_name in names {
                    if let Err(err) = repo.create_reference(
                        reference_name,
                        new_commit.get_oid(),
                        true,
                        "move branches",
                    ) {
                        branch_move_err = Some(eyre::eyre!(err));
                        break 'outer;
                    }
                    branch_moves.push((*old_oid, MaybeZeroOid::NonZero(*new_oid), reference_name));
                }
            }

            MaybeZeroOid::Zero => {
                for reference_name in names {
                    if reference_name == &main_branch_name {
                        // Hack? Never delete the main branch. We probably got here by syncing the
                        // main branch with the upstream version, but all main branch commits were
                        // skipped. For a regular branch, we would delete the branch, but for the
                        // main branch, we should update it to point directly to the upstream
                        // version.
                        let target_oid = match main_branch.get_upstream_branch_target()? {
                            Some(target_oid) => {
                                if let Err(err) = repo.create_reference(
                                    &main_branch_name,
                                    target_oid,
                                    true,
                                    "move main branch",
                                ) {
                                    branch_move_err = Some(eyre::eyre!(err));
                                    break 'outer;
                                }
                                MaybeZeroOid::NonZero(target_oid)
                            }
                            None => {
                                let mut main_branch_reference =
                                    repo.get_main_branch()?.into_reference();
                                if let Err(err) = main_branch_reference.delete() {
                                    branch_move_err = Some(eyre::eyre!(err));
                                    break 'outer;
                                }
                                MaybeZeroOid::Zero
                            }
                        };
                        branch_moves.push((*old_oid, target_oid, reference_name));
                    } else {
                        let branch_name = CategorizedReferenceName::new(reference_name);
                        match branch_name {
                            CategorizedReferenceName::RemoteBranch { .. }
                            | CategorizedReferenceName::OtherRef { .. } => {
                                warn!(?reference_name, "Not deleting non-local-branch reference");
                            }
                            CategorizedReferenceName::LocalBranch { .. } => {
                                let branch_name = branch_name.render_suffix();
                                match repo.find_branch(&branch_name, BranchType::Local) {
                                    Ok(Some(mut branch)) => {
                                        if let Err(err) = branch.delete() {
                                            branch_move_err = Some(eyre::eyre!(err));
                                            break 'outer;
                                        }
                                    }
                                    Ok(None) => {
                                        warn!(?branch_name, "Branch not found, not deleting")
                                    }
                                    Err(err) => {
                                        branch_move_err = Some(eyre::eyre!(err));
                                        break 'outer;
                                    }
                                };
                                branch_moves.push((*old_oid, MaybeZeroOid::Zero, reference_name));
                            }
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::format_collect)]
    let branch_moves_stdin: String = branch_moves
        .into_iter()
        .map(|(old_oid, new_oid, name)| {
            format!("{old_oid} {new_oid} {name}\n", name = name.as_str())
        })
        .collect();
    let branch_moves_stdin = BString::from(branch_moves_stdin);
    git_run_info.run_hook(
        effects,
        repo,
        "reference-transaction",
        event_tx_id,
        &["committed"],
        Some(branch_moves_stdin),
    )?;
    match branch_move_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// After a rebase, check out the appropriate new `HEAD`. This can be difficult
/// because the commit might have been rewritten, dropped, or have a branch
/// pointing to it which also needs to be checked out.
///
/// `skipped_head_updated_oid` is the caller's belief of what the new OID of
/// `HEAD` should be in the event that the original commit was skipped. If the
/// caller doesn't think that the previous `HEAD` commit was skipped, then they
/// should pass in `None`.
pub fn check_out_updated_head(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_log_db: &EventLogDb,
    event_tx_id: EventTransactionId,
    rewritten_oids: &HashMap<NonZeroOid, MaybeZeroOid>,
    previous_head_info: &ResolvedReferenceInfo,
    skipped_head_updated_oid: Option<NonZeroOid>,
    check_out_commit_options: &CheckOutCommitOptions,
) -> EyreExitOr<()> {
    let checkout_target: ResolvedReferenceInfo = match previous_head_info {
        ResolvedReferenceInfo {
            oid: None,
            reference_name: None,
        } => {
            // Head was unborn, so no need to check out a new branch.
            ResolvedReferenceInfo {
                oid: skipped_head_updated_oid,
                reference_name: None,
            }
        }

        ResolvedReferenceInfo {
            oid: None,
            reference_name: Some(reference_name),
        } => {
            // Head was unborn but a branch was checked out. Not sure if this
            // can happen, but if so, just use that branch.
            ResolvedReferenceInfo {
                oid: None,
                reference_name: Some(reference_name.clone()),
            }
        }

        ResolvedReferenceInfo {
            oid: Some(previous_head_oid),
            reference_name: None,
        } => {
            // No branch was checked out.
            match rewritten_oids.get(previous_head_oid) {
                Some(MaybeZeroOid::NonZero(oid)) => {
                    // This OID was rewritten, so check out the new version of the commit.
                    ResolvedReferenceInfo {
                        oid: Some(*oid),
                        reference_name: None,
                    }
                }
                Some(MaybeZeroOid::Zero) => {
                    // The commit was skipped. Get the new location for `HEAD`.
                    ResolvedReferenceInfo {
                        oid: skipped_head_updated_oid,
                        reference_name: None,
                    }
                }
                None => {
                    // This OID was not rewritten, so check it out again.
                    ResolvedReferenceInfo {
                        oid: Some(*previous_head_oid),
                        reference_name: None,
                    }
                }
            }
        }

        ResolvedReferenceInfo {
            oid: Some(_),
            reference_name: Some(reference_name),
        } => {
            // Find the reference at current time to see if it still exists.
            match repo.find_reference(reference_name)? {
                Some(reference) => {
                    // The branch moved, so we need to make sure that we are
                    // still checked out to it.
                    //
                    // * On-disk rebases will end with the branch pointing to
                    // the last rebase head, which may not be the `HEAD` commit
                    // before the rebase.
                    //
                    // * In-memory rebases will detach `HEAD` before proceeding,
                    // so we need to reattach it if necessary.
                    let oid = repo.resolve_reference(&reference)?.oid;
                    ResolvedReferenceInfo {
                        oid,
                        reference_name: Some(reference_name.clone()),
                    }
                }

                None => {
                    // The branch was deleted because it pointed to a skipped
                    // commit. Get the new location for `HEAD`.
                    ResolvedReferenceInfo {
                        oid: skipped_head_updated_oid,
                        reference_name: None,
                    }
                }
            }
        }
    };

    let head_info = repo.get_head_info()?;
    if head_info == checkout_target {
        return Ok(Ok(()));
    }

    let checkout_target: CheckoutTarget = match &checkout_target {
        ResolvedReferenceInfo {
            oid: None,
            reference_name: None,
        } => return Ok(Ok(())),

        ResolvedReferenceInfo {
            oid: Some(oid),
            reference_name: None,
        } => CheckoutTarget::Oid(*oid),

        ResolvedReferenceInfo {
            oid: _,
            reference_name: Some(reference_name),
        } => {
            // FIXME: we could check to see if the OIDs are the same and, if so,
            // reattach or detach `HEAD` manually without having to call `git checkout`.
            let checkout_target = match checkout_target.get_branch_name()? {
                Some(branch_name) => branch_name,
                None => reference_name.as_str(),
            };
            CheckoutTarget::Reference(ReferenceName::from(checkout_target))
        }
    };

    let result = check_out_commit(
        effects,
        git_run_info,
        repo,
        event_log_db,
        event_tx_id,
        Some(checkout_target),
        check_out_commit_options,
    )?;
    Ok(result)
}

/// What to suggest that the user do in order to resolve a merge conflict.
#[derive(Copy, Clone, Debug)]
pub enum MergeConflictRemediation {
    /// Indicate that the user should retry the merge operation (but with
    /// `--merge`).
    Retry,

    /// Indicate that the user should run `git restack --merge`.
    Restack,

    /// Indicate that the user should run `git move -m -s 'siblings(.)'`.
    Insert,
}

/// Information about a failure to merge that occurred while moving commits.
#[derive(Debug)]
pub enum FailedMergeInfo {
    /// A merge conflict occurred.
    Conflict {
        /// The OID of the commit that, when moved, caused a conflict.
        commit_oid: NonZeroOid,

        /// The paths which were in conflict.
        conflicting_paths: HashSet<PathBuf>,
    },

    /// A merge commit could not be rebased in memory.
    CannotRebaseMergeInMemory {
        /// The OID of the merge commit that could not be moved.
        commit_oid: NonZeroOid,
    },
}

impl FailedMergeInfo {
    /// Describe the merge conflict in a user-friendly way and advise to rerun
    /// with `--merge`.
    pub fn describe(
        &self,
        effects: &Effects,
        repo: &Repo,
        remediation: MergeConflictRemediation,
    ) -> eyre::Result<()> {
        match self {
            FailedMergeInfo::Conflict {
                commit_oid,
                conflicting_paths,
            } => {
                writeln!(
                    effects.get_output_stream(),
                    "This operation would cause a merge conflict:"
                )?;
                writeln!(
                    effects.get_output_stream(),
                    "{} ({}) {}",
                    effects.get_glyphs().bullet_point,
                    Pluralize {
                        determiner: None,
                        amount: conflicting_paths.len(),
                        unit: ("conflicting file", "conflicting files"),
                    },
                    effects.get_glyphs().render(
                        repo.friendly_describe_commit_from_oid(effects.get_glyphs(), *commit_oid)?
                    )?
                )?;
            }

            FailedMergeInfo::CannotRebaseMergeInMemory { commit_oid } => {
                writeln!(
                    effects.get_output_stream(),
                    "Merge commits currently can't be rebased in-memory."
                )?;
                writeln!(
                    effects.get_output_stream(),
                    "The merge commit was: {}",
                    effects.get_glyphs().render(
                        repo.friendly_describe_commit_from_oid(effects.get_glyphs(), *commit_oid)?
                    )?,
                )?;
            }
        }

        match remediation {
            MergeConflictRemediation::Retry => {
                writeln!(
                    effects.get_output_stream(),
                    "To resolve merge conflicts, retry this operation with the --merge option."
                )?;
            }
            MergeConflictRemediation::Restack => {
                writeln!(
                    effects.get_output_stream(),
                    "To resolve merge conflicts, run: git restack --merge"
                )?;
            }
            MergeConflictRemediation::Insert => {
                writeln!(
                    effects.get_output_stream(),
                    "To resolve merge conflicts, run: git move -m -s 'siblings(.)'"
                )?;
            }
        }

        Ok(())
    }
}

mod in_memory {
    use std::collections::HashMap;
    use std::fmt::Write;

    use bstr::{BString, ByteSlice};
    use eyre::Context;
    use tracing::{instrument, warn};

    use crate::core::effects::{Effects, OperationIcon, OperationType};
    use crate::core::eventlog::EventLogDb;
    use crate::core::gc::mark_commit_reachable;
    use crate::core::rewrite::execute::check_out_updated_head;
    use crate::core::rewrite::move_branches;
    use crate::core::rewrite::plan::{OidOrLabel, RebaseCommand, RebasePlan};
    use crate::git::{
        self, AmendFastOptions, CherryPickFastOptions, CreateCommitFastError, GitRunInfo,
        MaybeZeroOid, NonZeroOid, Repo,
    };
    use crate::util::EyreExitOr;

    use super::{ExecuteRebasePlanOptions, FailedMergeInfo};

    pub enum RebaseInMemoryResult {
        Succeeded {
            rewritten_oids: HashMap<NonZeroOid, MaybeZeroOid>,

            /// The new OID that `HEAD` should point to, based on the rebase.
            ///
            /// - This is only `None` if `HEAD` was unborn.
            /// - This doesn't capture if `HEAD` was pointing to a branch. The
            ///   caller will need to figure that out.
            new_head_oid: Option<NonZeroOid>,
        },
        MergeFailed(FailedMergeInfo),
    }

    #[instrument]
    pub fn rebase_in_memory(
        effects: &Effects,
        repo: &Repo,
        rebase_plan: &RebasePlan,
        options: &ExecuteRebasePlanOptions,
    ) -> eyre::Result<RebaseInMemoryResult> {
        if let Some(merge_commit_oid) =
            rebase_plan
                .commands
                .iter()
                .find_map(|command| match command {
                    RebaseCommand::Merge {
                        commit_oid,
                        commits_to_merge: _,
                    } => Some(commit_oid),
                    RebaseCommand::CreateLabel { .. }
                    | RebaseCommand::Reset { .. }
                    | RebaseCommand::Pick { .. }
                    | RebaseCommand::Replace { .. }
                    | RebaseCommand::Break
                    | RebaseCommand::RegisterExtraPostRewriteHook
                    | RebaseCommand::DetectEmptyCommit { .. }
                    | RebaseCommand::SkipUpstreamAppliedCommit { .. } => None,
                })
        {
            return Ok(RebaseInMemoryResult::MergeFailed(
                FailedMergeInfo::CannotRebaseMergeInMemory {
                    commit_oid: *merge_commit_oid,
                },
            ));
        }

        let ExecuteRebasePlanOptions {
            now,
            // Transaction ID will be passed to the `post-rewrite` hook via
            // environment variable.
            event_tx_id: _,
            preserve_timestamps,
            force_in_memory: _,
            force_on_disk: _,
            resolve_merge_conflicts: _, // May be needed once we can resolve merge conflicts in memory.
            check_out_commit_options: _, // Caller is responsible for checking out to new HEAD.
            sign_option,
        } = options;

        let mut current_oid = rebase_plan.first_dest_oid;
        let mut labels: HashMap<String, NonZeroOid> = HashMap::new();
        let mut rewritten_oids: HashMap<NonZeroOid, MaybeZeroOid> = HashMap::new();

        // Normally, we can determine the new `HEAD` OID by looking at the
        // rewritten commits. However, if `HEAD` pointed to a commit that was
        // skipped, then the rewritten OID is zero. In that case, we need to
        // delete the branch (responsibility of the caller) and choose a
        // different `HEAD` OID.
        let head_oid = repo.get_head_info()?.oid;
        let mut skipped_head_new_oid = None;
        let mut maybe_set_skipped_head_new_oid = |skipped_head_oid, current_oid| {
            if Some(skipped_head_oid) == head_oid {
                skipped_head_new_oid.get_or_insert(current_oid);
            }
        };

        let mut i = 0;
        let num_picks = rebase_plan
            .commands
            .iter()
            .filter(|command| match command {
                RebaseCommand::CreateLabel { .. }
                | RebaseCommand::Reset { .. }
                | RebaseCommand::Break
                | RebaseCommand::RegisterExtraPostRewriteHook
                | RebaseCommand::DetectEmptyCommit { .. } => false,
                RebaseCommand::Pick { .. }
                | RebaseCommand::Merge { .. }
                | RebaseCommand::Replace { .. }
                | RebaseCommand::SkipUpstreamAppliedCommit { .. } => true,
            })
            .count();
        let (effects, progress) = effects.start_operation(OperationType::RebaseCommits);

        let signer = git::get_signer(repo, sign_option)?;

        for command in rebase_plan.commands.iter() {
            match command {
                RebaseCommand::CreateLabel { label_name } => {
                    labels.insert(label_name.clone(), current_oid);
                }

                RebaseCommand::Reset {
                    target: OidOrLabel::Label(label_name),
                } => {
                    current_oid = match labels.get(label_name) {
                        Some(oid) => *oid,
                        None => eyre::bail!("BUG: no associated OID for label: {label_name}"),
                    };
                }

                RebaseCommand::Reset {
                    target: OidOrLabel::Oid(commit_oid),
                } => {
                    current_oid = match rewritten_oids.get(commit_oid) {
                        Some(MaybeZeroOid::NonZero(rewritten_oid)) => {
                            // HEAD has been rewritten.
                            *rewritten_oid
                        }
                        Some(MaybeZeroOid::Zero) | None => {
                            // Either HEAD was not rewritten, or it was but its
                            // associated commit was skipped. Either way, just
                            // use the current OID.
                            *commit_oid
                        }
                    };
                }

                RebaseCommand::Pick {
                    original_commit_oid,
                    commits_to_apply_oids,
                } => {
                    let current_commit = repo
                        .find_commit_or_fail(current_oid)
                        .wrap_err("Finding current commit")?;

                    let original_commit = repo
                        .find_commit_or_fail(*original_commit_oid)
                        .wrap_err("Finding commit to apply")?;
                    i += 1;

                    let commit_num = format!("[{i}/{num_picks}]");
                    progress.notify_progress(i, num_picks);

                    let commit_message = original_commit.get_message_raw();
                    let commit_message = commit_message.to_str().with_context(|| {
                        eyre::eyre!(
                            "Could not decode commit message for commit: {:?}",
                            original_commit_oid
                        )
                    })?;

                    let commit_author = original_commit.get_author();
                    let committer_signature = if *preserve_timestamps {
                        original_commit.get_committer()
                    } else {
                        original_commit.get_committer().update_timestamp(*now)?
                    };
                    let mut rebased_commit_oid = None;
                    let mut rebased_commit = None;

                    for commit_oid in commits_to_apply_oids.iter() {
                        let commit_to_apply = repo
                            .find_commit_or_fail(*commit_oid)
                            .wrap_err("Finding commit to apply")?;
                        let commit_description = effects
                            .get_glyphs()
                            .render(commit_to_apply.friendly_describe(effects.get_glyphs())?)?;

                        if commit_to_apply.get_parent_count() > 1 {
                            warn!(
                                ?commit_oid,
                                "BUG: Merge commit should have been detected during planning phase"
                            );
                            return Ok(RebaseInMemoryResult::MergeFailed(
                                FailedMergeInfo::CannotRebaseMergeInMemory {
                                    commit_oid: *commit_oid,
                                },
                            ));
                        };

                        progress.notify_status(
                            OperationIcon::InProgress,
                            format!("Applying patch for commit: {commit_description}"),
                        );

                        // Create a commit and then repeatedly amend & re-create it
                        // FIXME what #perf gains can be had by working directly on a tree?
                        // Is it even possible to repeatedly amend a tree and then commit
                        // it once at the end?

                        let maybe_tree = if rebased_commit.is_none() {
                            repo.cherry_pick_fast(
                                &commit_to_apply,
                                &current_commit,
                                &CherryPickFastOptions {
                                    reuse_parent_tree_if_possible: true,
                                },
                            )
                        } else {
                            repo.amend_fast(
                                &rebased_commit.expect("rebased commit should not be None"),
                                &AmendFastOptions::FromCommit {
                                    commit: commit_to_apply,
                                },
                            )
                        };
                        let commit_tree = match maybe_tree {
                            Ok(tree) => tree,
                            Err(CreateCommitFastError::MergeConflict { conflicting_paths }) => {
                                return Ok(RebaseInMemoryResult::MergeFailed(
                                    FailedMergeInfo::Conflict {
                                        commit_oid: *commit_oid,
                                        conflicting_paths,
                                    },
                                ))
                            }
                            Err(other) => eyre::bail!(other),
                        };

                        // this is the description of each fixup commit
                        // FIXME should we instead be using the description of the base commit?
                        // or use a different message altogether when squashing multiple commits?
                        progress.notify_status(
                            OperationIcon::InProgress,
                            format!("Committing to repository: {commit_description}"),
                        );
                        rebased_commit_oid = Some(
                            repo.create_commit(
                                &commit_author,
                                &committer_signature,
                                commit_message,
                                &commit_tree,
                                vec![&current_commit],
                                signer.as_deref(),
                            )
                            .wrap_err("Applying rebased commit")?,
                        );

                        rebased_commit = repo.find_commit(rebased_commit_oid.unwrap())?;
                    }

                    let rebased_commit_oid =
                        rebased_commit_oid.expect("rebased oid should not be None");
                    let commit_description =
                        effects
                            .get_glyphs()
                            .render(repo.friendly_describe_commit_from_oid(
                                effects.get_glyphs(),
                                rebased_commit_oid,
                            )?)?;

                    if rebased_commit
                        .expect("rebased commit should not be None")
                        .is_empty()
                    {
                        rewritten_oids.insert(*original_commit_oid, MaybeZeroOid::Zero);
                        maybe_set_skipped_head_new_oid(*original_commit_oid, current_oid);

                        writeln!(
                            effects.get_output_stream(),
                            "{commit_num} Skipped now-empty commit: {commit_description}",
                        )?;
                    } else {
                        rewritten_oids.insert(
                            *original_commit_oid,
                            MaybeZeroOid::NonZero(rebased_commit_oid),
                        );
                        for commit_oid in commits_to_apply_oids {
                            rewritten_oids
                                .insert(*commit_oid, MaybeZeroOid::NonZero(rebased_commit_oid));
                        }

                        current_oid = rebased_commit_oid;

                        writeln!(
                            effects.get_output_stream(),
                            "{commit_num} Committed as: {commit_description}"
                        )?;
                    }
                }

                RebaseCommand::Merge {
                    commit_oid,
                    commits_to_merge: _,
                } => {
                    warn!(
                        ?commit_oid,
                        "BUG: Merge commit without replacement should have been detected when starting in-memory rebase"
                    );
                    return Ok(RebaseInMemoryResult::MergeFailed(
                        FailedMergeInfo::CannotRebaseMergeInMemory {
                            commit_oid: *commit_oid,
                        },
                    ));
                }

                RebaseCommand::Replace {
                    commit_oid,
                    replacement_commit_oid,
                    parents,
                } => {
                    let original_commit = repo
                        .find_commit_or_fail(*commit_oid)
                        .wrap_err("Finding current commit")?;
                    let original_commit_description = effects
                        .get_glyphs()
                        .render(original_commit.friendly_describe(effects.get_glyphs())?)?;

                    i += 1;
                    let commit_num = format!("[{i}/{num_picks}]");
                    progress.notify_progress(i, num_picks);
                    progress.notify_status(
                        OperationIcon::InProgress,
                        format!("Replacing commit: {original_commit_description}"),
                    );

                    let replacement_commit = repo.find_commit_or_fail(*replacement_commit_oid)?;
                    let replacement_tree = replacement_commit.get_tree()?;
                    let replacement_message = replacement_commit.get_message_raw();
                    let replacement_commit_message =
                        replacement_message.to_str().with_context(|| {
                            eyre::eyre!(
                                "Could not decode commit message for replacement commit: {:?}",
                                replacement_commit
                            )
                        })?;

                    let replacement_commit_description = effects
                        .get_glyphs()
                        .render(replacement_commit.friendly_describe(effects.get_glyphs())?)?;
                    progress.notify_status(
                        OperationIcon::InProgress,
                        format!("Committing to repository: {replacement_commit_description}"),
                    );
                    let committer_signature = if *preserve_timestamps {
                        replacement_commit.get_committer()
                    } else {
                        replacement_commit.get_committer().update_timestamp(*now)?
                    };
                    let parents = {
                        let mut result = Vec::new();
                        for parent in parents {
                            let parent_oid = match parent {
                                OidOrLabel::Oid(oid) => *oid,
                                OidOrLabel::Label(label) => {
                                    let oid = labels.get(label).ok_or_else(|| {
                                        eyre::eyre!(
                                            "Label {label} could not be resolved to a commit"
                                        )
                                    })?;
                                    *oid
                                }
                            };
                            let parent_commit = repo.find_commit_or_fail(parent_oid)?;
                            result.push(parent_commit);
                        }
                        result
                    };
                    let rebased_commit_oid = repo
                        .create_commit(
                            &replacement_commit.get_author(),
                            &committer_signature,
                            replacement_commit_message,
                            &replacement_tree,
                            parents.iter().collect(),
                            signer.as_deref(),
                        )
                        .wrap_err("Applying rebased commit")?;

                    let commit_description =
                        effects
                            .get_glyphs()
                            .render(repo.friendly_describe_commit_from_oid(
                                effects.get_glyphs(),
                                rebased_commit_oid,
                            )?)?;
                    rewritten_oids.insert(*commit_oid, MaybeZeroOid::NonZero(rebased_commit_oid));
                    current_oid = rebased_commit_oid;

                    writeln!(
                        effects.get_output_stream(),
                        "{commit_num} Committed as: {commit_description}"
                    )?;
                }

                RebaseCommand::Break => {
                    eyre::bail!("`break` not supported for in-memory rebases");
                }

                RebaseCommand::SkipUpstreamAppliedCommit { commit_oid } => {
                    i += 1;
                    let commit_num = format!("[{i}/{num_picks}]");

                    let commit = repo.find_commit_or_fail(*commit_oid)?;
                    rewritten_oids.insert(*commit_oid, MaybeZeroOid::Zero);
                    maybe_set_skipped_head_new_oid(*commit_oid, current_oid);

                    let commit_description = commit.friendly_describe(effects.get_glyphs())?;
                    let commit_description = effects.get_glyphs().render(commit_description)?;
                    writeln!(
                        effects.get_output_stream(),
                        "{commit_num} Skipped commit (was already applied upstream): {commit_description}"
                    )?;
                }

                RebaseCommand::RegisterExtraPostRewriteHook
                | RebaseCommand::DetectEmptyCommit { .. } => {
                    // Do nothing. We'll carry out post-rebase operations after the
                    // in-memory rebase completes.
                }
            }
        }

        let new_head_oid: Option<NonZeroOid> = match head_oid {
            None => {
                // `HEAD` is unborn, so keep it that way.
                None
            }
            Some(head_oid) => {
                match rewritten_oids.get(&head_oid) {
                    Some(MaybeZeroOid::NonZero(new_head_oid)) => {
                        // `HEAD` was rewritten to this OID.
                        Some(*new_head_oid)
                    }
                    Some(MaybeZeroOid::Zero) => {
                        // `HEAD` was rewritten, but its associated commit was
                        // skipped. Use whatever saved new `HEAD` OID we have.
                        let new_head_oid = match skipped_head_new_oid {
                            Some(new_head_oid) => new_head_oid,
                            None => {
                                warn!(
                                    ?head_oid,
                                    "`HEAD` OID was rewritten to 0, but no skipped `HEAD` OID was set",
                                );
                                head_oid
                            }
                        };
                        Some(new_head_oid)
                    }
                    None => {
                        // The `HEAD` OID was not rewritten, so use its current value.
                        Some(head_oid)
                    }
                }
            }
        };
        Ok(RebaseInMemoryResult::Succeeded {
            rewritten_oids,
            new_head_oid,
        })
    }

    pub fn post_rebase_in_memory(
        effects: &Effects,
        git_run_info: &GitRunInfo,
        repo: &Repo,
        event_log_db: &EventLogDb,
        rewritten_oids: &HashMap<NonZeroOid, MaybeZeroOid>,
        skipped_head_updated_oid: Option<NonZeroOid>,
        options: &ExecuteRebasePlanOptions,
    ) -> EyreExitOr<()> {
        let ExecuteRebasePlanOptions {
            now: _,
            event_tx_id,
            preserve_timestamps: _,
            force_in_memory: _,
            force_on_disk: _,
            resolve_merge_conflicts: _,
            check_out_commit_options,
            sign_option: _,
        } = options;

        for new_oid in rewritten_oids.values() {
            if let MaybeZeroOid::NonZero(new_oid) = new_oid {
                mark_commit_reachable(repo, *new_oid)?;
            }
        }

        let head_info = repo.get_head_info()?;
        if head_info.oid.is_some() {
            // Avoid moving the branch which HEAD points to, or else the index will show
            // a lot of changes in the working copy.
            repo.detach_head(&head_info)?;
        }

        move_branches(effects, git_run_info, repo, *event_tx_id, rewritten_oids)?;

        // Call the `post-rewrite` hook only after moving branches so that we don't
        // produce a spurious abandoned-branch warning.
        #[allow(clippy::format_collect)]
        let post_rewrite_stdin: String = rewritten_oids
            .iter()
            .map(|(old_oid, new_oid)| format!("{old_oid} {new_oid}\n"))
            .collect();
        let post_rewrite_stdin = BString::from(post_rewrite_stdin);
        git_run_info.run_hook(
            effects,
            repo,
            "post-rewrite",
            *event_tx_id,
            &["rebase"],
            Some(post_rewrite_stdin),
        )?;

        let exit_code = check_out_updated_head(
            effects,
            git_run_info,
            repo,
            event_log_db,
            *event_tx_id,
            rewritten_oids,
            &head_info,
            skipped_head_updated_oid,
            check_out_commit_options,
        )?;
        Ok(exit_code)
    }
}

mod on_disk {
    use std::fmt::Write;

    use eyre::Context;
    use tracing::instrument;

    use crate::core::effects::{Effects, OperationType};
    use crate::core::rewrite::plan::RebaseCommand;
    use crate::core::rewrite::plan::RebasePlan;
    use crate::core::rewrite::rewrite_hooks::save_original_head_info;
    use crate::git::{GitRunInfo, Repo};

    use crate::util::ExitCode;

    use super::ExecuteRebasePlanOptions;

    pub enum Error {
        ChangedFilesInRepository,
        OperationAlreadyInProgress { operation_type: String },
    }

    fn write_rebase_state_to_disk(
        effects: &Effects,
        git_run_info: &GitRunInfo,
        repo: &Repo,
        rebase_plan: &RebasePlan,
        options: &ExecuteRebasePlanOptions,
    ) -> eyre::Result<Result<(), Error>> {
        let ExecuteRebasePlanOptions {
            now: _,
            event_tx_id: _,
            preserve_timestamps,
            force_in_memory: _,
            force_on_disk: _,
            resolve_merge_conflicts: _,
            check_out_commit_options: _, // Checkout happens after rebase has concluded.
            sign_option,
        } = options;

        let (effects, _progress) = effects.start_operation(OperationType::InitializeRebase);

        let head_info = repo.get_head_info()?;

        let current_operation_type = repo.get_current_operation_type();
        if let Some(current_operation_type) = current_operation_type {
            return Ok(Err(Error::OperationAlreadyInProgress {
                operation_type: current_operation_type.to_string(),
            }));
        }

        if repo.has_changed_files(&effects, git_run_info)? {
            return Ok(Err(Error::ChangedFilesInRepository));
        }

        let rebase_state_dir = repo.get_rebase_state_dir_path();
        std::fs::create_dir_all(&rebase_state_dir).wrap_err_with(|| {
            format!(
                "Creating rebase state directory at: {:?}",
                &rebase_state_dir
            )
        })?;

        // Mark this rebase as an interactive rebase. For whatever reason, if this
        // is not marked as an interactive rebase, then some rebase plans fail with
        // this error:
        //
        // ```
        // BUG: builtin/rebase.c:1178: Unhandled rebase type 1
        // ```
        let interactive_file_path = rebase_state_dir.join("interactive");
        std::fs::write(&interactive_file_path, "")
            .wrap_err_with(|| format!("Writing interactive to: {:?}", &interactive_file_path))?;

        if let Some(head_oid) = head_info.oid {
            let orig_head_file_path = repo.get_path().join("ORIG_HEAD");
            std::fs::write(&orig_head_file_path, head_oid.to_string())
                .wrap_err_with(|| format!("Writing `ORIG_HEAD` to: {:?}", &orig_head_file_path))?;

            // Confusingly, there is also a file at
            // `.git/rebase-merge/orig-head` (different from `.git/ORIG_HEAD`),
            // which seems to store the same thing.
            let rebase_orig_head_file_path = rebase_state_dir.join("orig-head");
            std::fs::write(&rebase_orig_head_file_path, head_oid.to_string()).wrap_err_with(
                || format!("Writing `orig-head` to: {:?}", &rebase_orig_head_file_path),
            )?;

            // `head-name` contains the name of the branch which will be reset
            // to point to the OID contained in `orig-head` when the rebase is
            // aborted.
            let head_name_file_path = rebase_state_dir.join("head-name");
            std::fs::write(
                &head_name_file_path,
                head_info
                    .reference_name
                    .as_ref()
                    .map(|reference_name| reference_name.as_str())
                    .unwrap_or("detached HEAD"),
            )
            .wrap_err_with(|| format!("Writing head-name to: {:?}", &head_name_file_path))?;

            save_original_head_info(repo, &head_info)?;

            // Dummy `head` file. We will `reset` to the appropriate commit as soon as
            // we start the rebase.
            let rebase_merge_head_file_path = rebase_state_dir.join("head");
            std::fs::write(
                &rebase_merge_head_file_path,
                rebase_plan.first_dest_oid.to_string(),
            )
            .wrap_err_with(|| format!("Writing head to: {:?}", &rebase_merge_head_file_path))?;
        }

        // Dummy `onto` file. We may be rebasing onto a set of unrelated
        // nodes in the same operation, so there may not be a single "onto" node to
        // refer to.
        let onto_file_path = rebase_state_dir.join("onto");
        std::fs::write(&onto_file_path, rebase_plan.first_dest_oid.to_string()).wrap_err_with(
            || {
                format!(
                    "Writing onto {:?} to: {:?}",
                    &rebase_plan.first_dest_oid, &onto_file_path
                )
            },
        )?;

        if rebase_plan.commands.iter().any(|command| match command {
            RebaseCommand::Pick {
                original_commit_oid,
                commits_to_apply_oids,
            } => !commits_to_apply_oids
                .iter()
                .any(|oid| oid == original_commit_oid),
            _ => false,
        }) {
            eyre::bail!("Not implemented: replacing commits in an on disk rebase");
        }

        let todo_file_path = rebase_state_dir.join("git-rebase-todo");
        #[allow(clippy::format_collect)]
        std::fs::write(
            &todo_file_path,
            rebase_plan
                .commands
                .iter()
                .map(|command| format!("{}\n", command.to_rebase_command()))
                .collect::<String>(),
        )
        .wrap_err_with(|| {
            format!(
                "Writing `git-rebase-todo` to: {:?}",
                todo_file_path.as_path()
            )
        })?;

        let gpg_sign_opt_path = rebase_state_dir.join("gpg_sign_opt");
        if let Some(sign_flag) = sign_option.as_rebase_flag(repo)? {
            std::fs::write(&gpg_sign_opt_path, sign_flag).wrap_err_with(|| {
                format!(
                    "Writing `gpg_sign_opt` to: {:?}",
                    gpg_sign_opt_path.as_path()
                )
            })?;
        }

        let end_file_path = rebase_state_dir.join("end");
        std::fs::write(
            end_file_path.as_path(),
            format!("{}\n", rebase_plan.commands.len()),
        )
        .wrap_err_with(|| format!("Writing `end` to: {:?}", end_file_path.as_path()))?;

        // Corresponds to the `--empty=keep` flag. We'll drop the commits later once
        // we find out that they're empty.
        let keep_redundant_commits_file_path = rebase_state_dir.join("keep_redundant_commits");
        std::fs::write(&keep_redundant_commits_file_path, "").wrap_err_with(|| {
            format!(
                "Writing `keep_redundant_commits` to: {:?}",
                &keep_redundant_commits_file_path
            )
        })?;

        if *preserve_timestamps {
            let cdate_is_adate_file_path = rebase_state_dir.join("cdate_is_adate");
            std::fs::write(&cdate_is_adate_file_path, "").wrap_err_with(|| {
                format!(
                    "Writing `cdate_is_adate` option file to: {:?}",
                    &cdate_is_adate_file_path
                )
            })?;
        }

        // Make sure we don't move around the current branch unintentionally. If it
        // actually needs to be moved, then it will be moved as part of the
        // post-rebase operations.
        if head_info.oid.is_some() {
            repo.detach_head(&head_info)?;
        }

        Ok(Ok(()))
    }

    /// Rebase on-disk. We don't use `git2`'s `Rebase` machinery because it ends up
    /// being too slow.
    ///
    /// Note that this calls `git rebase`, which may fail (e.g. if there are
    /// merge conflicts). The exit code is then propagated to the caller.
    #[instrument]
    pub fn rebase_on_disk(
        effects: &Effects,
        git_run_info: &GitRunInfo,
        repo: &Repo,
        rebase_plan: &RebasePlan,
        options: &ExecuteRebasePlanOptions,
    ) -> eyre::Result<Result<ExitCode, Error>> {
        let ExecuteRebasePlanOptions {
            // `git rebase` will make its own timestamp.
            now: _,
            event_tx_id,
            preserve_timestamps: _,
            force_in_memory: _,
            force_on_disk: _,
            resolve_merge_conflicts: _,
            check_out_commit_options: _, // Checkout happens after rebase has concluded.
            sign_option: _,
        } = options;

        match write_rebase_state_to_disk(effects, git_run_info, repo, rebase_plan, options)? {
            Ok(()) => {}
            Err(err) => return Ok(Err(err)),
        };

        writeln!(
            effects.get_output_stream(),
            "Calling Git for on-disk rebase..."
        )?;
        match git_run_info.run(effects, Some(*event_tx_id), &["rebase", "--continue"])? {
            Ok(()) => Ok(Ok(ExitCode::success())),
            Err(err) => Ok(Ok(err)),
        }
    }
}

/// Options to use when executing a `RebasePlan`.
#[derive(Clone, Debug)]
pub struct ExecuteRebasePlanOptions {
    /// The time which should be recorded for this event.
    pub now: SystemTime,

    /// The transaction ID for this event.
    pub event_tx_id: EventTransactionId,

    /// If `true`, any rewritten commits will keep the same authored and
    /// committed timestamps. If `false`, the committed timestamps will be updated
    /// to the current time.
    pub preserve_timestamps: bool,

    /// Force an in-memory rebase (as opposed to an on-disk rebase).
    pub force_in_memory: bool,

    /// Force an on-disk rebase (as opposed to an in-memory rebase).
    pub force_on_disk: bool,

    /// Whether or not an attempt should be made to resolve merge conflicts,
    /// rather than failing-fast.
    pub resolve_merge_conflicts: bool,

    /// If `HEAD` was moved, the options for checking out the new `HEAD` commit.
    pub check_out_commit_options: CheckOutCommitOptions,

    /// GPG-sign commits.
    pub sign_option: SignOption,
}

/// The result of executing a rebase plan.
#[must_use]
#[derive(Debug)]
pub enum ExecuteRebasePlanResult {
    /// The rebase operation succeeded.
    Succeeded {
        /// Mapping from old OID to new/rewritten OID. Will always be empty for on disk rebases.
        rewritten_oids: Option<HashMap<NonZeroOid, MaybeZeroOid>>,
    },

    /// The rebase operation encounter a failure to merge, and it was not
    /// requested to try to resolve it.
    DeclinedToMerge {
        /// Information about the merge failure that occurred.
        failed_merge_info: FailedMergeInfo,
    },

    /// The rebase operation failed.
    Failed {
        /// The exit code to exit with. (This value may have been obtained from
        /// a subcommand invocation.)
        exit_code: ExitCode,
    },
}

/// Execute the provided rebase plan. Returns the exit status (zero indicates
/// success).
pub fn execute_rebase_plan(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_log_db: &EventLogDb,
    rebase_plan: &RebasePlan,
    options: &ExecuteRebasePlanOptions,
) -> eyre::Result<ExecuteRebasePlanResult> {
    let ExecuteRebasePlanOptions {
        now: _,
        event_tx_id: _,
        preserve_timestamps: _,
        force_in_memory,
        force_on_disk,
        resolve_merge_conflicts,
        check_out_commit_options: _,
        sign_option: _,
    } = options;

    if !force_on_disk {
        use in_memory::*;
        writeln!(
            effects.get_output_stream(),
            "Attempting rebase in-memory..."
        )?;

        let failed_merge_info = match rebase_in_memory(effects, repo, rebase_plan, options)? {
            RebaseInMemoryResult::MergeFailed(failed_merge_info) => failed_merge_info,

            RebaseInMemoryResult::Succeeded {
                rewritten_oids,
                new_head_oid,
            } => {
                // Ignore the return code, as it probably indicates that the
                // checkout failed (which might happen if the user has changes
                // which don't merge cleanly). The user can resolve that
                // themselves.
                match post_rebase_in_memory(
                    effects,
                    git_run_info,
                    repo,
                    event_log_db,
                    &rewritten_oids,
                    new_head_oid,
                    options,
                )? {
                    Ok(()) => {}
                    Err(_exit_code) => {
                        // FIXME: we may still want to propagate the exit code to the
                        // caller.
                    }
                }

                writeln!(effects.get_output_stream(), "In-memory rebase succeeded.")?;
                return Ok(ExecuteRebasePlanResult::Succeeded {
                    rewritten_oids: Some(rewritten_oids),
                });
            }
        };

        if !resolve_merge_conflicts {
            return Ok(ExecuteRebasePlanResult::DeclinedToMerge { failed_merge_info });
        }

        // The rebase has failed at this point, decide whether or not to try
        // again with an on-disk rebase.
        if *force_in_memory {
            writeln!(
                effects.get_output_stream(),
                "Aborting since an in-memory rebase was requested."
            )?;
            return Ok(ExecuteRebasePlanResult::DeclinedToMerge { failed_merge_info });
        } else {
            writeln!(
                effects.get_output_stream(),
                "Failed to merge in-memory, trying again on-disk..."
            )?;
        }
    }

    if !force_in_memory {
        use on_disk::*;
        match rebase_on_disk(effects, git_run_info, repo, rebase_plan, options)? {
            Ok(exit_code) if exit_code.is_success() => {
                return Ok(ExecuteRebasePlanResult::Succeeded {
                    rewritten_oids: None,
                });
            }
            Ok(exit_code) => return Ok(ExecuteRebasePlanResult::Failed { exit_code }),
            Err(Error::ChangedFilesInRepository) => {
                write!(
                    effects.get_output_stream(),
                    "\
This operation would modify the working copy, but you have uncommitted changes
in your working copy which might be overwritten as a result.
Commit your changes and then try again.
"
                )?;
                return Ok(ExecuteRebasePlanResult::Failed {
                    exit_code: ExitCode(1),
                });
            }
            Err(Error::OperationAlreadyInProgress { operation_type }) => {
                writeln!(
                    effects.get_output_stream(),
                    "A {operation_type} operation is already in progress."
                )?;
                writeln!(
                    effects.get_output_stream(),
                    "Run git {operation_type} --continue or git {operation_type} --abort to resolve it and proceed."
                )?;
                return Ok(ExecuteRebasePlanResult::Failed {
                    exit_code: ExitCode(1),
                });
            }
        }
    }

    eyre::bail!("Both force_in_memory and force_on_disk were requested, but these options conflict")
}
