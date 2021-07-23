use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::time::SystemTime;

use anyhow::Context;
use os_str_bytes::OsStrBytes;

use crate::core::eventlog::EventTransactionId;
use crate::core::formatting::{printable_styled_string, Glyphs};
use crate::git::{GitRunInfo, MaybeZeroOid, NonZeroOid, Repo};

use super::plan::RebasePlan;

/// Given a list of rewritten OIDs, move the branches attached to those OIDs
/// from their old commits to their new commits. Invoke the
/// `reference-transaction` hook when done.
pub fn move_branches<'a>(
    git_run_info: &GitRunInfo,
    repo: &'a Repo,
    event_tx_id: EventTransactionId,
    rewritten_oids_map: &'a HashMap<NonZeroOid, MaybeZeroOid>,
) -> anyhow::Result<()> {
    let branch_oid_to_names = repo.get_branch_oid_to_names()?;

    // We may experience an error in the case of a branch move. Ideally, we
    // would use `git2::Transaction::commit`, which stops the transaction at the
    // first error, but we don't know which references we successfully committed
    // in that case. Instead, we just do things non-atomically and record which
    // ones succeeded. See https://github.com/libgit2/libgit2/issues/5918
    let mut branch_moves: Vec<(NonZeroOid, MaybeZeroOid, &OsStr)> = Vec::new();
    let mut branch_move_err: Option<anyhow::Error> = None;
    'outer: for (old_oid, names) in branch_oid_to_names.iter() {
        let new_oid = match rewritten_oids_map.get(&old_oid) {
            Some(new_oid) => new_oid,
            None => continue,
        };
        let mut names: Vec<_> = names.iter().collect();
        // Sort for determinism in tests.
        names.sort_unstable();
        match new_oid {
            MaybeZeroOid::NonZero(new_oid) => {
                let new_commit = match repo.find_commit(*new_oid) {
                    Ok(Some(commit)) => commit,
                    Ok(None) => {
                        branch_move_err = Some(anyhow::anyhow!(
                            "Could not find newly-rewritten commit with old OID: {:?}, new OID: {:?}",
                            old_oid,
                            new_oid
                        ));
                        break 'outer;
                    }
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
                            log::warn!("Reference not found, not deleting: {:?}", name)
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
        .with_context(|| "Encoding branch moves stdin")?;
    let branch_moves_stdin = OsString::from(branch_moves_stdin);
    git_run_info.run_hook(
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

    use anyhow::Context;
    use fn_error_context::context;
    use indicatif::{ProgressBar, ProgressStyle};

    use crate::commands::gc::mark_commit_reachable;
    use crate::core::formatting::{printable_styled_string, Glyphs};
    use crate::core::rewrite::move_branches;
    use crate::core::rewrite::plan::{RebaseCommand, RebasePlan};
    use crate::git::{GitRunInfo, MaybeZeroOid, NonZeroOid, Repo};

    use super::ExecuteRebasePlanOptions;

    pub enum RebaseInMemoryResult {
        Succeeded {
            rewritten_oids: Vec<(NonZeroOid, MaybeZeroOid)>,
        },
        CannotRebaseMergeCommit {
            commit_oid: NonZeroOid,
        },
        MergeConflict {
            commit_oid: NonZeroOid,
        },
    }

    #[context("Rebasing in memory")]
    pub fn rebase_in_memory(
        glyphs: &Glyphs,
        repo: &Repo,
        rebase_plan: &RebasePlan,
        options: &ExecuteRebasePlanOptions,
    ) -> anyhow::Result<RebaseInMemoryResult> {
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

        let mut i = 0;
        let num_picks = rebase_plan
            .commands
            .iter()
            .filter(|command| match command {
                RebaseCommand::CreateLabel { .. }
                | RebaseCommand::ResetToLabel { .. }
                | RebaseCommand::ResetToOid { .. }
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

                RebaseCommand::ResetToLabel { label_name } => {
                    current_oid = match labels.get(label_name) {
                        Some(oid) => *oid,
                        None => anyhow::bail!("BUG: no associated OID for label: {}", label_name),
                    };
                }

                RebaseCommand::ResetToOid { commit_oid } => {
                    current_oid = *commit_oid;
                }

                RebaseCommand::Pick { commit_oid } => {
                    let current_commit = repo.find_commit(current_oid).with_context(|| {
                        format!("Finding current commit by OID: {:?}", current_oid)
                    })?;
                    let current_commit = match current_commit {
                        Some(commit) => commit,
                        None => {
                            anyhow::bail!(
                                "Unable to find current commit with OID: {:?}",
                                current_oid
                            )
                        }
                    };
                    let commit_to_apply = repo.find_commit(*commit_oid).with_context(|| {
                        format!("Finding commit to apply by OID: {:?}", commit_oid)
                    })?;
                    let commit_to_apply = match commit_to_apply {
                        Some(commit) => commit,
                        None => {
                            anyhow::bail!(
                                "Unable to find commit to apply with OID: {:?}",
                                current_oid
                            )
                        }
                    };
                    i += 1;

                    let commit_description =
                        printable_styled_string(glyphs, commit_to_apply.friendly_describe()?)?;
                    let commit_num = format!("[{}/{}]", i, num_picks);
                    let progress_template = format!("{} {{spinner}} {{wide_msg}}", commit_num);
                    let progress = ProgressBar::new_spinner();
                    progress.set_style(
                        ProgressStyle::default_spinner().template(&progress_template.trim()),
                    );
                    progress.set_message("Starting");
                    progress.enable_steady_tick(100);

                    if commit_to_apply.get_parent_count() > 1 {
                        return Ok(RebaseInMemoryResult::CannotRebaseMergeCommit {
                            commit_oid: *commit_oid,
                        });
                    };

                    progress
                        .set_message(format!("Applying patch for commit: {}", commit_description));
                    let mut rebased_index =
                        repo.cherrypick_commit(&commit_to_apply, &current_commit, 0, None)?;

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
                        .with_context(|| "Converting index to tree")?;
                    let commit_tree = match repo.find_tree(commit_tree_oid)? {
                        Some(tree) => tree,
                        None => anyhow::bail!(
                            "Could not find freshly-written tree for OID: {:?}",
                            commit_tree_oid
                        ),
                    };
                    let commit_message = commit_to_apply.get_message_raw()?;
                    let commit_message = match commit_message.to_str() {
                        Some(message) => message,
                        None => anyhow::bail!(
                            "Could not decode commit message for commit: {:?}",
                            commit_oid
                        ),
                    };

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
                        .with_context(|| "Applying rebased commit")?;

                    let rebased_commit = match repo
                        .find_commit(rebased_commit_oid)
                        .with_context(|| "Looking up just-rebased commit")?
                    {
                        Some(commit) => commit,
                        None => {
                            anyhow::bail!(
                                "Could not find just-rebased commit: {:?}",
                                rebased_commit_oid
                            )
                        }
                    };
                    let commit_description = printable_styled_string(
                        glyphs,
                        repo.friendly_describe_commit_from_oid(rebased_commit_oid)?,
                    )?;
                    if rebased_commit.is_empty() {
                        rewritten_oids.push((*commit_oid, MaybeZeroOid::Zero));
                        progress.finish_and_clear();
                        println!(
                            "[{}/{}] Skipped now-empty commit: {}",
                            i, num_picks, commit_description
                        );
                    } else {
                        rewritten_oids
                            .push((*commit_oid, MaybeZeroOid::NonZero(rebased_commit_oid)));
                        current_oid = rebased_commit_oid;
                        progress.finish_and_clear();
                        println!("{} Committed as: {}", commit_num, commit_description);
                    }
                }

                RebaseCommand::SkipUpstreamAppliedCommit { commit_oid } => {
                    let progress = ProgressBar::new_spinner();
                    i += 1;
                    let commit_num = format!("[{}/{}]", i, num_picks);
                    let progress_template = format!("{} {{spinner}} {{wide_msg}}", commit_num);
                    progress.set_style(
                        ProgressStyle::default_spinner().template(&progress_template.trim()),
                    );
                    let commit = match repo.find_commit(*commit_oid)? {
                        Some(commit) => commit,
                        None => anyhow::bail!("Could not find commit: {:?}", commit_oid),
                    };
                    let commit_description = commit.friendly_describe()?;
                    let commit_description = printable_styled_string(glyphs, commit_description)?;
                    rewritten_oids.push((*commit_oid, MaybeZeroOid::Zero));
                    progress.finish_and_clear();
                    println!(
                        "{} Skipped commit (was already applied upstream): {}",
                        commit_num, commit_description
                    );
                }

                RebaseCommand::RegisterExtraPostRewriteHook
                | RebaseCommand::DetectEmptyCommit { .. } => {
                    // Do nothing. We'll carry out post-rebase operations after the
                    // in-memory rebase completes.
                }
            }
        }

        Ok(RebaseInMemoryResult::Succeeded { rewritten_oids })
    }

    pub fn post_rebase_in_memory(
        git_run_info: &GitRunInfo,
        repo: &Repo,
        rewritten_oids: &[(NonZeroOid, MaybeZeroOid)],
        options: &ExecuteRebasePlanOptions,
    ) -> anyhow::Result<isize> {
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
            head_info.detach_head()?;
        }

        move_branches(git_run_info, repo, *event_tx_id, &rewritten_oids_map)?;

        // Call the `post-rewrite` hook only after moving branches so that we don't
        // produce a spurious abandoned-branch warning.
        let post_rewrite_stdin: String = rewritten_oids
            .iter()
            .map(|(old_oid, new_oid)| format!("{} {}\n", old_oid.to_string(), new_oid.to_string()))
            .collect();
        let post_rewrite_stdin = OsString::from(post_rewrite_stdin);
        git_run_info.run_hook(
            repo,
            "post-rewrite",
            *event_tx_id,
            &["rebase"],
            Some(post_rewrite_stdin),
        )?;

        if let Some(head_oid) = head_info.oid {
            if let Some(new_head_oid) = rewritten_oids_map.get(&head_oid) {
                let head_target = match head_info.get_branch_name() {
                    Some(head_branch) => head_branch.to_string(),
                    None => match new_head_oid {
                        MaybeZeroOid::NonZero(new_head_oid) => new_head_oid.to_string(),
                        MaybeZeroOid::Zero => format!("{}^", head_oid.to_string()),
                    },
                };
                let result = git_run_info.run(Some(*event_tx_id), &["checkout", &head_target])?;
                if result != 0 {
                    return Ok(result);
                }
            }
        }

        Ok(0)
    }
}

mod on_disk {
    use anyhow::Context;
    use fn_error_context::context;
    use indicatif::ProgressBar;

    use crate::core::rewrite::plan::RebasePlan;
    use crate::git::{GitRunInfo, Repo};

    use super::ExecuteRebasePlanOptions;

    /// Rebase on-disk. We don't use `git2`'s `Rebase` machinery because it ends up
    /// being too slow.
    #[context("Rebasing on disk")]
    pub fn rebase_on_disk(
        git_run_info: &GitRunInfo,
        repo: &Repo,
        rebase_plan: &RebasePlan,
        options: &ExecuteRebasePlanOptions,
    ) -> anyhow::Result<isize> {
        let ExecuteRebasePlanOptions {
            // `git rebase` will make its own timestamp.
            now: _,
            event_tx_id,
            preserve_timestamps,
            force_in_memory: _,
            force_on_disk: _,
        } = options;

        let progress = ProgressBar::new_spinner();
        progress.enable_steady_tick(100);
        progress.set_message("Initializing rebase");

        let head_info = repo.get_head_info()?;

        let current_operation_type = repo.get_current_operation_type();
        if let Some(current_operation_type) = current_operation_type {
            progress.finish_and_clear();
            println!(
                "A {} operation is already in progress.",
                current_operation_type
            );
            println!(
                "Run git {0} --continue or git {0} --abort to resolve it and proceed.",
                current_operation_type
            );
            return Ok(1);
        }

        let rebase_state_dir = repo.get_rebase_state_dir_path();
        std::fs::create_dir_all(&rebase_state_dir).with_context(|| {
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
            .with_context(|| format!("Writing interactive to: {:?}", &interactive_file_path))?;

        if head_info.oid.is_some() {
            let repo_head_file_path = repo.get_path().join("HEAD");
            let orig_head_file_path = repo.get_path().join("ORIG_HEAD");
            std::fs::copy(&repo_head_file_path, &orig_head_file_path)
                .with_context(|| format!("Copying `HEAD` to: {:?}", &orig_head_file_path))?;
            // `head-name` appears to be purely for UX concerns. Git will warn if the
            // file isn't found.
            let head_name_file_path = rebase_state_dir.join("head-name");
            std::fs::write(
                &head_name_file_path,
                head_info.get_branch_name().unwrap_or("detached HEAD"),
            )
            .with_context(|| format!("Writing head-name to: {:?}", &head_name_file_path))?;

            // Dummy `head` file. We will `reset` to the appropriate commit as soon as
            // we start the rebase.
            let rebase_merge_head_file_path = rebase_state_dir.join("head");
            std::fs::write(
                &rebase_merge_head_file_path,
                rebase_plan.first_dest_oid.to_string(),
            )
            .with_context(|| format!("Writing head to: {:?}", &rebase_merge_head_file_path))?;
        }

        // Dummy `onto` file. We may be rebasing onto a set of unrelated
        // nodes in the same operation, so there may not be a single "onto" node to
        // refer to.
        let onto_file_path = rebase_state_dir.join("onto");
        std::fs::write(&onto_file_path, rebase_plan.first_dest_oid.to_string()).with_context(
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
        .with_context(|| {
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
        .with_context(|| format!("Writing `end` to: {:?}", end_file_path.as_path()))?;

        // Corresponds to the `--empty=keep` flag. We'll drop the commits later once
        // we find out that they're empty.
        let keep_redundant_commits_file_path = rebase_state_dir.join("keep_redundant_commits");
        std::fs::write(&keep_redundant_commits_file_path, "").with_context(|| {
            format!(
                "Writing `keep_redundant_commits` to: {:?}",
                &keep_redundant_commits_file_path
            )
        })?;

        if *preserve_timestamps {
            let cdate_is_adate_file_path = rebase_state_dir.join("cdate_is_adate");
            std::fs::write(&cdate_is_adate_file_path, "")
                .with_context(|| "Writing `cdate_is_adate` option file")?;
        }

        // Make sure we don't move around the current branch unintentionally. If it
        // actually needs to be moved, then it will be moved as part of the
        // post-rebase operations.
        if head_info.oid.is_some() {
            head_info.detach_head()?;
        }

        progress.finish_and_clear();
        println!("Calling Git for on-disk rebase...");
        let result = git_run_info.run(Some(*event_tx_id), &["rebase", "--continue"])?;
        Ok(result)
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
    glyphs: &Glyphs,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    rebase_plan: &RebasePlan,
    options: &ExecuteRebasePlanOptions,
) -> anyhow::Result<isize> {
    let ExecuteRebasePlanOptions {
        now: _,
        event_tx_id: _,
        preserve_timestamps: _,
        force_in_memory,
        force_on_disk,
    } = options;

    if !force_on_disk {
        use in_memory::*;
        println!("Attempting rebase in-memory...");
        match rebase_in_memory(glyphs, &repo, &rebase_plan, &options)? {
            RebaseInMemoryResult::Succeeded { rewritten_oids } => {
                post_rebase_in_memory(git_run_info, repo, &rewritten_oids, &options)?;
                println!("In-memory rebase succeeded.");
                return Ok(0);
            }
            RebaseInMemoryResult::CannotRebaseMergeCommit { commit_oid } => {
                println!(
                    "Merge commits currently can't be rebased with `git move`. The merge commit was: {}",
                    printable_styled_string(glyphs, repo.friendly_describe_commit_from_oid(commit_oid)?)?,
                );
                return Ok(1);
            }
            RebaseInMemoryResult::MergeConflict { commit_oid } => {
                if *force_in_memory {
                    println!(
                        "Merge conflict. The conflicting commit was: {}",
                        printable_styled_string(
                            glyphs,
                            repo.friendly_describe_commit_from_oid(commit_oid)?,
                        )?,
                    );
                    println!("Aborting since an in-memory rebase was requested.");
                    return Ok(1);
                } else {
                    println!(
                        "Merge conflict, falling back to rebase on-disk. The conflicting commit was: {}",
                        printable_styled_string(glyphs, repo.friendly_describe_commit_from_oid(commit_oid)?)?,
                    );
                }
            }
        }
    }

    if !force_in_memory {
        use on_disk::*;
        let result = rebase_on_disk(git_run_info, repo, &rebase_plan, &options)?;
        return Ok(result);
    }

    anyhow::bail!(
        "Both force_in_memory and force_on_disk were requested, but these options conflict"
    )
}
