use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt::Write;
use std::time::SystemTime;

use eyre::Context;
use os_str_bytes::OsStrBytes;
use tracing::warn;

use crate::core::eventlog::EventTransactionId;
use crate::core::formatting::printable_styled_string;
use crate::git::{GitRunInfo, MaybeZeroOid, NonZeroOid, Repo};
use crate::tui::Effects;

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
    let branch_oid_to_names = repo.get_branch_oid_to_names()?;

    // We may experience an error in the case of a branch move. Ideally, we
    // would use `git2::Transaction::commit`, which stops the transaction at the
    // first error, but we don't know which references we successfully committed
    // in that case. Instead, we just do things non-atomically and record which
    // ones succeeded. See https://github.com/libgit2/libgit2/issues/5918
    let mut branch_moves: Vec<(NonZeroOid, MaybeZeroOid, &OsStr)> = Vec::new();
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
                        "Could not find newly-rewritten commit with old OID: {:?}, new OID: {:?}",
                        old_oid, new_oid,
                    )
                }) {
                    Ok(commit) => commit,
                    Err(err) => {
                        branch_move_err = Some(err);
                        break 'outer;
                    }
                };

                for name in names {
                    if let Err(err) =
                        repo.create_reference(name, new_commit.get_oid(), true, "move branches")
                    {
                        branch_move_err = Some(err);
                        break 'outer;
                    }
                    branch_moves.push((*old_oid, MaybeZeroOid::NonZero(*new_oid), name));
                }
            }

            MaybeZeroOid::Zero => {
                for name in names {
                    match repo.find_reference(name) {
                        Ok(Some(mut reference)) => {
                            if let Err(err) = reference.delete() {
                                branch_move_err = Some(err);
                                break 'outer;
                            }
                        }
                        Ok(None) => {
                            warn!(?name, "Reference not found, not deleting")
                        }
                        Err(err) => {
                            branch_move_err = Some(err);
                            break 'outer;
                        }
                    };
                    branch_moves.push((*old_oid, MaybeZeroOid::Zero, name));
                }
            }
        }
    }

    let branch_moves_stdin: Vec<u8> = branch_moves
        .into_iter()
        .flat_map(|(old_oid, new_oid, name)| {
            let mut line = Vec::new();
            line.extend(old_oid.to_string().as_bytes());
            line.push(b' ');
            line.extend(new_oid.to_string().as_bytes());
            line.push(b' ');
            line.extend(name.to_raw_bytes().iter());
            line.push(b'\n');
            line
        })
        .collect();
    let branch_moves_stdin = OsStrBytes::from_raw_bytes(branch_moves_stdin)
        .wrap_err_with(|| "Encoding branch moves stdin")?;
    let branch_moves_stdin = OsString::from(branch_moves_stdin);
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

mod in_memory {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::fmt::Write;

    use eyre::Context;
    use indicatif::{ProgressBar, ProgressStyle};
    use tracing::{instrument, warn};

    use crate::commands::gc::mark_commit_reachable;
    use crate::core::formatting::printable_styled_string;
    use crate::core::rewrite::move_branches;
    use crate::core::rewrite::plan::{OidOrLabel, RebaseCommand, RebasePlan};
    use crate::git::{GitRunInfo, MaybeZeroOid, NonZeroOid, Repo};
    use crate::tui::Effects;

    use super::ExecuteRebasePlanOptions;

    pub enum RebaseInMemoryResult {
        Succeeded {
            rewritten_oids: Vec<(NonZeroOid, MaybeZeroOid)>,

            /// The new OID that `HEAD` should point to, based on the rebase.
            ///
            /// - This is only `None` if `HEAD` was unborn.
            /// - This doesn't capture if `HEAD` was pointing to a branch. The
            /// caller will need to figure that out.
            new_head_oid: Option<NonZeroOid>,
        },
        CannotRebaseMergeCommit {
            commit_oid: NonZeroOid,
        },
        MergeConflict {
            commit_oid: NonZeroOid,
        },
    }

    #[instrument]
    pub fn rebase_in_memory(
        effects: &Effects,
        repo: &Repo,
        rebase_plan: &RebasePlan,
        options: &ExecuteRebasePlanOptions,
    ) -> eyre::Result<RebaseInMemoryResult> {
        let ExecuteRebasePlanOptions {
            now,
            // Transaction ID will be passed to the `post-rewrite` hook via
            // environment variable.
            event_tx_id: _,
            preserve_timestamps,
            force_in_memory: _,
            force_on_disk: _,
        } = options;

        let mut current_oid = rebase_plan.first_dest_oid;
        let mut labels: HashMap<String, NonZeroOid> = HashMap::new();
        let mut rewritten_oids: Vec<(NonZeroOid, MaybeZeroOid)> = Vec::new();

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
                | RebaseCommand::RegisterExtraPostRewriteHook
                | RebaseCommand::DetectEmptyCommit { .. } => false,
                RebaseCommand::Pick { .. } | RebaseCommand::SkipUpstreamAppliedCommit { .. } => {
                    true
                }
            })
            .count();

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
                        None => eyre::bail!("BUG: no associated OID for label: {}", label_name),
                    };
                }

                RebaseCommand::Reset {
                    target: OidOrLabel::Oid(commit_oid),
                } => {
                    current_oid = *commit_oid;
                }

                RebaseCommand::Pick { commit_oid } => {
                    let current_commit = repo
                        .find_commit_or_fail(current_oid)
                        .wrap_err_with(|| "Finding current commit")?;
                    let commit_to_apply = repo
                        .find_commit_or_fail(*commit_oid)
                        .wrap_err_with(|| "Finding commit to apply")?;
                    i += 1;

                    let commit_description = printable_styled_string(
                        effects.get_glyphs(),
                        commit_to_apply.friendly_describe()?,
                    )?;
                    let commit_num = format!("[{}/{}]", i, num_picks);
                    let progress_template = format!("{} {{spinner}} {{wide_msg}}", commit_num);
                    let progress = ProgressBar::new_spinner();
                    progress.set_style(
                        ProgressStyle::default_spinner().template(progress_template.trim()),
                    );
                    progress.set_message("Starting");
                    progress.enable_steady_tick(100);

                    if commit_to_apply.get_parent_count() > 1 {
                        warn!(
                            ?commit_oid,
                            "BUG: Merge commit should have been detected during planning phase"
                        );
                        return Ok(RebaseInMemoryResult::CannotRebaseMergeCommit {
                            commit_oid: *commit_oid,
                        });
                    };

                    progress
                        .set_message(format!("Applying patch for commit: {}", commit_description));
                    let mut rebased_index =
                        repo.cherrypick_commit(&commit_to_apply, &current_commit, 0)?;

                    progress.set_message(format!(
                        "Checking for merge conflicts: {}",
                        commit_description
                    ));
                    if rebased_index.has_conflicts() {
                        return Ok(RebaseInMemoryResult::MergeConflict {
                            commit_oid: *commit_oid,
                        });
                    }

                    progress.set_message(format!(
                        "Writing commit data to disk: {}",
                        commit_description
                    ));
                    let commit_tree_oid = repo
                        .write_index_to_tree(&mut rebased_index)
                        .wrap_err_with(|| "Converting index to tree")?;
                    let commit_tree = repo.find_tree(commit_tree_oid)?.ok_or_else(|| {
                        eyre::eyre!(
                            "Could not find freshly-written tree for OID: {:?}",
                            commit_tree_oid
                        )
                    })?;
                    let commit_message = commit_to_apply.get_message_raw()?;
                    let commit_message = commit_message.to_str().ok_or_else(|| {
                        eyre::eyre!(
                            "Could not decode commit message for commit: {:?}",
                            commit_oid
                        )
                    })?;

                    progress
                        .set_message(format!("Committing to repository: {}", commit_description));
                    let committer_signature = if *preserve_timestamps {
                        commit_to_apply.get_committer()
                    } else {
                        commit_to_apply.get_committer().update_timestamp(*now)?
                    };
                    let rebased_commit_oid = repo
                        .create_commit(
                            None,
                            &commit_to_apply.get_author(),
                            &committer_signature,
                            commit_message,
                            &commit_tree,
                            &[&current_commit],
                        )
                        .wrap_err_with(|| "Applying rebased commit")?;

                    let rebased_commit = repo
                        .find_commit_or_fail(rebased_commit_oid)
                        .wrap_err_with(|| "Looking up just-rebased commit")?;
                    let commit_description = printable_styled_string(
                        effects.get_glyphs(),
                        repo.friendly_describe_commit_from_oid(rebased_commit_oid)?,
                    )?;
                    if rebased_commit.is_empty() {
                        rewritten_oids.push((*commit_oid, MaybeZeroOid::Zero));
                        maybe_set_skipped_head_new_oid(*commit_oid, current_oid);

                        progress.finish_and_clear();
                        writeln!(
                            effects.get_output_stream(),
                            "[{}/{}] Skipped now-empty commit: {}",
                            i,
                            num_picks,
                            commit_description
                        )?;
                    } else {
                        rewritten_oids
                            .push((*commit_oid, MaybeZeroOid::NonZero(rebased_commit_oid)));
                        current_oid = rebased_commit_oid;

                        progress.finish_and_clear();
                        writeln!(
                            effects.get_output_stream(),
                            "{} Committed as: {}",
                            commit_num,
                            commit_description
                        )?;
                    }
                }

                RebaseCommand::SkipUpstreamAppliedCommit { commit_oid } => {
                    let progress = ProgressBar::new_spinner();
                    i += 1;
                    let commit_num = format!("[{}/{}]", i, num_picks);
                    let progress_template = format!("{} {{spinner}} {{wide_msg}}", commit_num);
                    progress.set_style(
                        ProgressStyle::default_spinner().template(progress_template.trim()),
                    );

                    let commit = repo.find_commit_or_fail(*commit_oid)?;
                    rewritten_oids.push((*commit_oid, MaybeZeroOid::Zero));
                    maybe_set_skipped_head_new_oid(*commit_oid, current_oid);

                    progress.finish_and_clear();
                    let commit_description = commit.friendly_describe()?;
                    let commit_description =
                        printable_styled_string(effects.get_glyphs(), commit_description)?;
                    writeln!(
                        effects.get_output_stream(),
                        "{} Skipped commit (was already applied upstream): {}",
                        commit_num,
                        commit_description
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
                let new_head_oid = rewritten_oids.iter().find_map(|(source_oid, dest_oid)| {
                    if *source_oid == head_oid {
                        Some(*dest_oid)
                    } else {
                        None
                    }
                });
                match new_head_oid {
                    Some(MaybeZeroOid::NonZero(new_head_oid)) => {
                        // `HEAD` was rewritten to this OID.
                        Some(new_head_oid)
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
        rewritten_oids: &[(NonZeroOid, MaybeZeroOid)],
        new_head_oid: Option<NonZeroOid>,
        options: &ExecuteRebasePlanOptions,
    ) -> eyre::Result<isize> {
        let ExecuteRebasePlanOptions {
            now: _,
            event_tx_id,
            preserve_timestamps: _,
            force_in_memory: _,
            force_on_disk: _,
        } = options;

        // Note that if an OID has been mapped to multiple other OIDs, then the last
        // mapping wins. (This corresponds to the last applied rebase operation.)
        let rewritten_oids_map: HashMap<NonZeroOid, MaybeZeroOid> =
            rewritten_oids.iter().copied().collect();

        for new_oid in rewritten_oids_map.values() {
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

        move_branches(
            effects,
            git_run_info,
            repo,
            *event_tx_id,
            &rewritten_oids_map,
        )?;

        // Call the `post-rewrite` hook only after moving branches so that we don't
        // produce a spurious abandoned-branch warning.
        let post_rewrite_stdin: String = rewritten_oids
            .iter()
            .map(|(old_oid, new_oid)| format!("{} {}\n", old_oid.to_string(), new_oid.to_string()))
            .collect();
        let post_rewrite_stdin = OsString::from(post_rewrite_stdin);
        git_run_info.run_hook(
            effects,
            repo,
            "post-rewrite",
            *event_tx_id,
            &["rebase"],
            Some(post_rewrite_stdin),
        )?;

        let (previous_head_oid, new_head_oid) = match head_info.oid {
            None => {
                // `HEAD` was unborn, so don't check anything out.
                return Ok(0);
            }
            Some(previous_head_oid) => {
                let new_head_oid = match new_head_oid {
                    Some(new_head_oid) => new_head_oid,
                    None => eyre::bail!(
                        "`None` provided for `new_head_oid`,
                        but it should have been `Some`
                        because the previous `HEAD` OID was not `None`: {:?}",
                        previous_head_oid
                    ),
                };
                (previous_head_oid, new_head_oid)
            }
        };
        let head_target = match (
            head_info.get_branch_name(),
            rewritten_oids_map.get(&previous_head_oid),
        ) {
            (Some(head_branch), Some(MaybeZeroOid::NonZero(_))) => {
                // The `HEAD` branch has been moved above. Check it out now.
                head_branch.to_string()
            }
            (Some(_), Some(MaybeZeroOid::Zero)) => {
                // The `HEAD` branch was deleted, so check out whatever the
                // caller says is the new appropriate `HEAD` OID.
                new_head_oid.to_string()
            }
            (Some(head_branch), None) => {
                // The `HEAD` commit was not rewritten, but we detached it
                // from its branch, so check its branch out again.
                head_branch.to_string()
            }
            (None, _) => {
                // We don't have to worry about a branch, so just use whatever
                // we've been told is the new `HEAD` OID.
                new_head_oid.to_string()
            }
        };

        let result = git_run_info.run(effects, Some(*event_tx_id), &["checkout", &head_target])?;
        if result != 0 {
            return Ok(result);
        }

        Ok(0)
    }
}

mod on_disk {
    use std::fmt::Write;

    use eyre::Context;
    use tracing::instrument;

    use crate::core::rewrite::plan::RebasePlan;
    use crate::git::{GitRunInfo, MaybeZeroOid, Repo};
    use crate::tui::{Effects, OperationType};

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

        if head_info.oid.is_some() {
            let repo_head_file_path = repo.get_path().join("HEAD");
            let orig_head_file_path = repo.get_path().join("ORIG_HEAD");
            std::fs::copy(&repo_head_file_path, &orig_head_file_path)
                .wrap_err_with(|| format!("Copying `HEAD` to: {:?}", &orig_head_file_path))?;

            // Confusingly, there is also a file at
            // `.git/rebase-merge/orig-head` (different from `.git/ORIG_HEAD`),
            // which stores only the OID of the original `HEAD` commit.
            //
            // It's used by Git to rebase the originally-checked out branch.
            // However, we don't use it for that purpose; instead, we use it to
            // decide what commit we need to check out after the rebase
            // operation has completed successfully.
            let rebase_orig_head_oid: MaybeZeroOid = head_info.oid.into();
            let rebase_orig_head_file_path = rebase_state_dir.join("orig-head");
            std::fs::write(
                &rebase_orig_head_file_path,
                rebase_orig_head_oid.to_string(),
            )
            .wrap_err_with(|| {
                format!("Writing `orig-head` to: {:?}", &rebase_orig_head_file_path)
            })?;

            // `head-name` appears to be purely for UX concerns. Git will warn if the
            // file isn't found.
            let head_name_file_path = rebase_state_dir.join("head-name");
            std::fs::write(
                &head_name_file_path,
                head_info.get_branch_name().unwrap_or("detached HEAD"),
            )
            .wrap_err_with(|| format!("Writing head-name to: {:?}", &head_name_file_path))?;

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

        let todo_file_path = rebase_state_dir.join("git-rebase-todo");
        std::fs::write(
            &todo_file_path,
            rebase_plan
                .commands
                .iter()
                .map(|command| format!("{}\n", command.to_string()))
                .collect::<String>(),
        )
        .wrap_err_with(|| {
            format!(
                "Writing `git-rebase-todo` to: {:?}",
                todo_file_path.as_path()
            )
        })?;

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
            std::fs::write(&cdate_is_adate_file_path, "")
                .wrap_err_with(|| "Writing `cdate_is_adate` option file")?;
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
    ) -> eyre::Result<Result<isize, Error>> {
        let ExecuteRebasePlanOptions {
            // `git rebase` will make its own timestamp.
            now: _,
            event_tx_id,
            preserve_timestamps: _,
            force_in_memory: _,
            force_on_disk: _,
        } = options;

        match write_rebase_state_to_disk(effects, git_run_info, repo, rebase_plan, options)? {
            Ok(()) => {}
            Err(err) => return Ok(Err(err)),
        };

        writeln!(
            effects.get_output_stream(),
            "Calling Git for on-disk rebase..."
        )?;
        let exit_code = git_run_info.run(effects, Some(*event_tx_id), &["rebase", "--continue"])?;
        Ok(Ok(exit_code))
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
}

/// Execute the provided rebase plan. Returns the exit status (zero indicates
/// success).
pub fn execute_rebase_plan(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    rebase_plan: &RebasePlan,
    options: &ExecuteRebasePlanOptions,
) -> eyre::Result<isize> {
    let ExecuteRebasePlanOptions {
        now: _,
        event_tx_id: _,
        preserve_timestamps: _,
        force_in_memory,
        force_on_disk,
    } = options;

    if !force_on_disk {
        use in_memory::*;
        writeln!(
            effects.get_output_stream(),
            "Attempting rebase in-memory..."
        )?;
        match rebase_in_memory(effects, repo, rebase_plan, options)? {
            RebaseInMemoryResult::Succeeded {
                rewritten_oids,
                new_head_oid,
            } => {
                post_rebase_in_memory(
                    effects,
                    git_run_info,
                    repo,
                    &rewritten_oids,
                    new_head_oid,
                    options,
                )?;
                writeln!(effects.get_output_stream(), "In-memory rebase succeeded.")?;
                return Ok(0);
            }
            RebaseInMemoryResult::CannotRebaseMergeCommit { commit_oid } => {
                writeln!(effects.get_output_stream(),
                    "Merge commits currently can't be rebased with `git move`. The merge commit was: {}",
                    printable_styled_string(effects.get_glyphs(), repo.friendly_describe_commit_from_oid(commit_oid)?)?,
                )?;
                return Ok(1);
            }
            RebaseInMemoryResult::MergeConflict { commit_oid } => {
                if *force_in_memory {
                    writeln!(
                        effects.get_output_stream(),
                        "Merge conflict. The conflicting commit was: {}",
                        printable_styled_string(
                            effects.get_glyphs(),
                            repo.friendly_describe_commit_from_oid(commit_oid)?,
                        )?,
                    )?;
                    writeln!(
                        effects.get_output_stream(),
                        "Aborting since an in-memory rebase was requested."
                    )?;
                    return Ok(1);
                } else {
                    writeln!(effects.get_output_stream(),
                        "Merge conflict, falling back to rebase on-disk. The conflicting commit was: {}",
                        printable_styled_string(effects.get_glyphs(), repo.friendly_describe_commit_from_oid(commit_oid)?)?,
                    )?;
                }
            }
        }
    }

    if !force_in_memory {
        use on_disk::*;
        match rebase_on_disk(effects, git_run_info, repo, rebase_plan, options)? {
            Ok(exit_code) => return Ok(exit_code),
            Err(Error::ChangedFilesInRepository) => {
                write!(
                    effects.get_output_stream(),
                    "\
This operation would modify the working copy, but you have uncommitted changes
in your working copy which might be overwritten as a result.
Commit your changes and then try again.
"
                )?;
                return Ok(1);
            }
            Err(Error::OperationAlreadyInProgress { operation_type }) => {
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
                return Ok(1);
            }
        }
    }

    eyre::bail!("Both force_in_memory and force_on_disk were requested, but these options conflict")
}
