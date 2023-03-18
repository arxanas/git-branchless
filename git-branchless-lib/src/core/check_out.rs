//! Handle checking out commits on disk.

use std::ffi::{OsStr, OsString};
use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;
use eyre::Context;
use itertools::Itertools;
use tracing::instrument;

use crate::core::config::get_auto_switch_branches;
use crate::git::{
    update_index, CategorizedReferenceName, GitRunInfo, MaybeZeroOid, NonZeroOid, ReferenceName,
    Repo, Stage, UpdateIndexCommand, WorkingCopySnapshot,
};
use crate::util::EyreExitOr;

use super::config::get_undo_create_snapshots;
use super::effects::Effects;
use super::eventlog::{Event, EventLogDb, EventTransactionId};
use super::repo_ext::{RepoExt, RepoReferencesSnapshot};

/// An entity to check out.
#[derive(Clone, Debug)]
pub enum CheckoutTarget {
    /// A commit addressed directly by OID.
    Oid(NonZeroOid),

    /// A reference. If the reference is a branch, then the branch will be
    /// checked out.
    Reference(ReferenceName),

    /// The type of checkout target is not known, as it was provided from the
    /// user and we haven't resolved it ourselves.
    Unknown(String),
}

/// Options for checking out a commit.
#[derive(Clone, Debug)]
pub struct CheckOutCommitOptions {
    /// Additional arguments to pass to `git checkout`.
    pub additional_args: Vec<OsString>,

    /// Use `git reset` rather than `git checkout`; that is, leave the index and
    /// working copy unchanged, and just adjust the `HEAD` pointer.
    pub reset: bool,

    /// Whether or not to render the smartlog after the checkout has completed.
    pub render_smartlog: bool,
}

impl Default for CheckOutCommitOptions {
    fn default() -> Self {
        Self {
            additional_args: Default::default(),
            reset: false,
            render_smartlog: true,
        }
    }
}

fn maybe_get_branch_name(
    current_target: Option<String>,
    oid: Option<NonZeroOid>,
    repo: &Repo,
) -> eyre::Result<Option<String>> {
    let RepoReferencesSnapshot {
        head_oid,
        branch_oid_to_names,
        ..
    } = repo.get_references_snapshot()?;
    let oid = match current_target {
        Some(_) => oid,
        None => head_oid,
    };
    if current_target.is_some()
        && ((head_oid.is_some() && head_oid == oid)
            || current_target == head_oid.map(|o| o.to_string()))
    {
        // Don't try to checkout the branch if we aren't actually checking anything new out.
        return Ok(current_target);
    }

    // Determine if the oid corresponds to exactly a single branch. If so,
    // check that out directly.
    match oid {
        Some(oid) => match branch_oid_to_names.get(&oid) {
            Some(branch_names) => match branch_names.iter().exactly_one() {
                Ok(branch_name) => {
                    // To remove the `refs/heads/` prefix
                    let name = CategorizedReferenceName::new(branch_name);
                    Ok(Some(name.remove_prefix()?))
                }
                Err(_) => Ok(current_target),
            },
            None => Ok(current_target),
        },
        None => Ok(current_target),
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
    target: Option<CheckoutTarget>,
    options: &CheckOutCommitOptions,
) -> EyreExitOr<()> {
    let CheckOutCommitOptions {
        additional_args,
        reset,
        render_smartlog,
    } = options;

    let (target, oid) = match target {
        None => (None, None),
        Some(CheckoutTarget::Reference(reference_name)) => {
            let categorized_target = CategorizedReferenceName::new(&reference_name);
            (Some(categorized_target.remove_prefix()?), None)
        }
        Some(CheckoutTarget::Oid(oid)) => (Some(oid.to_string()), Some(oid)),
        Some(CheckoutTarget::Unknown(target)) => (Some(target), None),
    };

    if get_undo_create_snapshots(repo)? {
        create_snapshot(effects, git_run_info, repo, event_log_db, event_tx_id)?;
    }

    let target = if get_auto_switch_branches(repo)? {
        maybe_get_branch_name(target, oid, repo)?
    } else {
        target
    };

    if *reset {
        if let Some(target) = &target {
            git_run_info.run(effects, Some(event_tx_id), &["reset", target])??;
        }
    }

    let checkout_args = {
        let mut args = vec![OsStr::new("checkout")];
        if let Some(target) = &target {
            args.push(OsStr::new(target.as_str()));
        }
        args.extend(additional_args.iter().map(OsStr::new));
        args
    };
    if let Err(err) = git_run_info.run(effects, Some(event_tx_id), checkout_args.as_slice())? {
        writeln!(
            effects.get_output_stream(),
            "{}",
            effects.get_glyphs().render(StyledString::styled(
                match target {
                    Some(target) => format!("Failed to check out commit: {target}"),
                    None => "Failed to check out commit".to_string(),
                },
                BaseColor::Red.light()
            ))?
        )?;
        return Ok(Err(err));
    }

    // Determine if we currently have a snapshot checked out, and, if so,
    // attempt to restore it.
    {
        let head_info = repo.get_head_info()?;
        if let Some(head_oid) = head_info.oid {
            let head_commit = repo.find_commit_or_fail(head_oid)?;
            if let Some(snapshot) = WorkingCopySnapshot::try_from_base_commit(repo, &head_commit)? {
                restore_snapshot(effects, git_run_info, repo, event_tx_id, &snapshot)??;
            }
        }
    }

    if *render_smartlog {
        git_run_info.run_direct_no_wrapping(Some(event_tx_id), &["branchless", "smartlog"])??;
    }
    Ok(Ok(()))
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
        ref_name: head_info.reference_name,
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
) -> EyreExitOr<()> {
    writeln!(
        effects.get_error_stream(),
        "branchless: restoring from snapshot"
    )?;

    // Discard any working copy changes. The caller is responsible for having
    // snapshotted them if necessary.
    git_run_info
        .run(effects, Some(event_tx_id), &["reset", "--hard", "HEAD"])
        .wrap_err("Discarding working copy changes")??;

    // Check out the unstaged changes. Note that we don't call `git reset --hard
    // <target>` directly as part of the previous step, and instead do this
    // two-step process. This second `git checkout` is so that untracked files
    // don't get thrown away as part of checking out the snapshot, but instead
    // abort the procedure.
    git_run_info
        .run(
            effects,
            Some(event_tx_id),
            &["checkout", &snapshot.commit_unstaged.get_oid().to_string()],
        )
        // FIXME: it might be worth attempting to un-check-out this commit?
        .wrap_err("Checking out unstaged changes (fail if conflict)")??;

    // Restore any unstaged changes. They're already present in the working
    // copy, so we just have to adjust `HEAD`.
    match &snapshot.head_commit {
        Some(head_commit) => {
            git_run_info
                .run(
                    effects,
                    Some(event_tx_id),
                    &["reset", &head_commit.get_oid().to_string()],
                )
                .wrap_err("Update HEAD for unstaged changes")??;
        }
        None => {
            // Do nothing. The branch, if any, will be restored later below.
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
        git_run_info
            .run(
                effects,
                Some(event_tx_id),
                &["update-ref", ref_name.as_str(), &head_oid.to_string()],
            )
            .context("Restoring snapshot branch")??;

        git_run_info
            .run(
                effects,
                Some(event_tx_id),
                &["symbolic-ref", "HEAD", ref_name.as_str()],
            )
            .context("Checking out snapshot branch")??;
    }

    Ok(Ok(()))
}
