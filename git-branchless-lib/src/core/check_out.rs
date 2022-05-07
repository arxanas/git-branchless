//! Handle checking out commits on disk.

use std::ffi::{OsStr, OsString};
use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;
use eyre::Context;
use tracing::instrument;

use crate::git::{
    update_index, CategorizedReferenceName, GitRunInfo, MaybeZeroOid, Repo, Stage,
    UpdateIndexCommand, WorkingCopySnapshot,
};
use crate::util::ExitCode;

use super::config::get_undo_create_snapshots;
use super::effects::Effects;
use super::eventlog::{Event, EventLogDb, EventTransactionId};
use super::formatting::printable_styled_string;

/// Options for checking out a commit.
#[derive(Clone, Debug)]
pub struct CheckOutCommitOptions {
    /// Additional arguments to pass to `git checkout`.
    pub additional_args: Vec<OsString>,

    /// Whether or not to render the smartlog after the checkout has completed.
    pub render_smartlog: bool,
}

impl Default for CheckOutCommitOptions {
    fn default() -> Self {
        Self {
            additional_args: Default::default(),
            render_smartlog: true,
        }
    }
}

/// Checks out the requested commit. If the operation succeeds, then displays
/// the new smartlog. Otherwise displays a warning message.
#[instrument]
pub fn check_out_commit(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_log_db: &EventLogDb,
    event_tx_id: EventTransactionId,
    target: Option<impl AsRef<OsStr> + std::fmt::Debug>,
    options: &CheckOutCommitOptions,
) -> eyre::Result<ExitCode> {
    let CheckOutCommitOptions {
        additional_args,
        render_smartlog,
    } = options;

    let target = match target {
        None => None,
        Some(target) => {
            let categorized_target = CategorizedReferenceName::new(target.as_ref());
            Some(categorized_target.remove_prefix()?)
        }
    };

    if get_undo_create_snapshots(repo)? {
        create_snapshot(effects, git_run_info, repo, event_log_db, event_tx_id)?;
    }

    let args = {
        let mut args = vec![OsStr::new("checkout")];
        if let Some(target) = &target {
            args.push(target);
        }
        args.extend(additional_args.iter().map(OsStr::new));
        args
    };
    let exit_code = git_run_info.run(effects, Some(event_tx_id), args.as_slice())?;

    if !exit_code.is_success() {
        writeln!(
            effects.get_output_stream(),
            "{}",
            printable_styled_string(
                effects.get_glyphs(),
                StyledString::styled(
                    match target {
                        Some(target) =>
                            format!("Failed to check out commit: {}", target.to_string_lossy()),
                        None => "Failed to check out commit".to_string(),
                    },
                    BaseColor::Red.light()
                )
            )?
        )?;
        return Ok(exit_code);
    }

    // Determine if we currently have a snapshot checked out, and, if so,
    // attempt to restore it.
    {
        let head_info = repo.get_head_info()?;
        if let Some(head_oid) = head_info.oid {
            let head_commit = repo.find_commit_or_fail(head_oid)?;
            if let Some(snapshot) = WorkingCopySnapshot::try_from_base_commit(repo, &head_commit)? {
                let exit_code =
                    restore_snapshot(effects, git_run_info, repo, event_tx_id, &snapshot)?;
                if !exit_code.is_success() {
                    return Ok(exit_code);
                }
            }
        }
    }

    if *render_smartlog {
        let exit_code =
            git_run_info.run_direct_no_wrapping(Some(event_tx_id), &["branchless", "smartlog"])?;
        Ok(exit_code)
    } else {
        Ok(exit_code)
    }
}

/// Create a working copy snapshot containing the working copy's current contents.
///
/// The working copy contents are not changed by this operation. That is, the
/// caller would be responsible for discarding local changes (which might or
/// might not be the natural next step for the operation).
pub fn create_snapshot<'repo>(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &'repo Repo,
    event_log_db: &EventLogDb,
    event_tx_id: EventTransactionId,
) -> eyre::Result<WorkingCopySnapshot<'repo>> {
    writeln!(
        effects.get_error_stream(),
        "branchless: creating working copy snapshot"
    )?;

    let head_info = repo.get_head_info()?;
    let index = repo.get_index()?;
    let (snapshot, _status) =
        repo.get_status(effects, git_run_info, &index, &head_info, Some(event_tx_id))?;
    event_log_db.add_events(vec![Event::WorkingCopySnapshot {
        timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64(),
        event_tx_id,
        head_oid: MaybeZeroOid::from(head_info.oid),
        commit_oid: snapshot.base_commit.get_oid(),
        ref_name: head_info.reference_name.map(|name| name.into_owned()),
    }])?;
    Ok(snapshot)
}

/// Restore the given snapshot's contents into the working copy.
///
/// All tracked working copy contents are **discarded**, so the caller should
/// take a snapshot of them first, or otherwise ensure that the user's work is
/// not lost.
///
/// If there are untracked changes in the working copy, they are left intact,
/// *unless* they would conflict with the working copy snapshot contents. In
/// that case, the operation is aborted.
pub fn restore_snapshot(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_tx_id: EventTransactionId,
    snapshot: &WorkingCopySnapshot,
) -> eyre::Result<ExitCode> {
    writeln!(
        effects.get_error_stream(),
        "branchless: restoring from snapshot"
    )?;

    // Discard any working copy changes. The caller is responsible for having
    // snapshotted them if necessary.
    let exit_code = git_run_info
        .run(effects, Some(event_tx_id), &["reset", "--hard", "HEAD"])
        .wrap_err("Discarding working copy changes")?;
    if !exit_code.is_success() {
        return Ok(exit_code);
    }

    // Check out the unstaged changes. Note that we don't call `git reset --hard
    // <target>` directly as part of the previous step, and instead do this
    // two-step process. This second `git checkout` is so that untracked files
    // don't get thrown away as part of checking out the snapshot, but instead
    // abort the procedure.
    let exit_code = git_run_info
        .run(
            effects,
            Some(event_tx_id),
            &["checkout", &snapshot.commit_unstaged.get_oid().to_string()],
        )
        .wrap_err("Checking out unstaged changes (fail if conflict)")?;
    if !exit_code.is_success() {
        // FIXME: it might be worth attempting to un-check-out this commit?
        return Ok(exit_code);
    }

    // Restore any unstaged changes. They're already present in the working
    // copy, so we just have to adjust `HEAD`.
    match &snapshot.head_commit {
        Some(head_commit) => {
            let exit_code = git_run_info
                .run(
                    effects,
                    Some(event_tx_id),
                    &["reset", &head_commit.get_oid().to_string()],
                )
                .wrap_err("Update HEAD for unstaged changes")?;
            if !exit_code.is_success() {
                return Ok(exit_code);
            }
        }
        None => {
            unimplemented!("Cannot restore snapshot of commit with no HEAD");
        }
    }

    // Check out the staged changes.
    let update_index_script = {
        let mut commands = Vec::new();
        for (stage, commit) in [
            (Stage::Stage0, &snapshot.commit_stage0),
            (Stage::Stage1, &snapshot.commit_stage1),
            (Stage::Stage2, &snapshot.commit_stage2),
            (Stage::Stage3, &snapshot.commit_stage3),
        ] {
            let changed_paths = match repo.get_paths_touched_by_commit(commit)? {
                Some(changed_paths) => changed_paths,
                None => continue,
            };
            for path in changed_paths {
                let tree = commit.get_tree()?;
                let tree_entry = tree.get_path(&path)?;

                let is_deleted = tree_entry.is_none();
                if is_deleted {
                    commands.push(UpdateIndexCommand::Delete { path: path.clone() })
                }

                if let Some(tree_entry) = tree_entry {
                    commands.push(UpdateIndexCommand::Update {
                        path,
                        stage,
                        mode: tree_entry.get_filemode(),
                        oid: tree_entry.get_oid(),
                    })
                }
            }
        }
        commands
    };
    let index = repo.get_index()?;
    update_index(
        git_run_info,
        repo,
        &index,
        event_tx_id,
        &update_index_script,
    )?;

    // If the snapshot had a branch, then we've just checked out the branch to
    // the base commit, but it should point to the head commit.  Move it there.
    if let Some(ref_name) = &snapshot.head_reference_name {
        let head_oid = match &snapshot.head_commit {
            Some(head_commit) => MaybeZeroOid::NonZero(head_commit.get_oid()),
            None => MaybeZeroOid::Zero,
        };
        let exit_code = git_run_info
            .run(
                effects,
                Some(event_tx_id),
                &["update-ref", ref_name, &head_oid.to_string()],
            )
            .context("Restoring snapshot branch")?;
        if !exit_code.is_success() {
            return Ok(exit_code);
        }

        let exit_code = git_run_info
            .run(
                effects,
                Some(event_tx_id),
                &["symbolic-ref", "HEAD", ref_name],
            )
            .context("Checking out snapshot branch")?;
        if !exit_code.is_success() {
            return Ok(exit_code);
        }
    }

    Ok(ExitCode(0))
}
