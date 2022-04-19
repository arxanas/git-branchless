//! Handle checking out commits on disk.

use std::ffi::{OsStr, OsString};
use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;
use tracing::instrument;

use crate::git::{CategorizedReferenceName, GitRunInfo, MaybeZeroOid, Repo, WorkingCopySnapshot};

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
) -> eyre::Result<isize> {
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

    // Create new working copy snapshot.
    {
        let head_info = repo.get_head_info()?;
        let index = repo.get_index()?;
        let (snapshot, _status) =
            repo.get_status(git_run_info, &index, &head_info, Some(event_tx_id))?;
        event_log_db.add_events(vec![Event::WorkingCopySnapshot {
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64(),
            event_tx_id,
            head_oid: MaybeZeroOid::from(head_info.oid),
            commit_oid: snapshot.base_commit.get_oid(),
            ref_name: head_info.reference_name.map(|name| name.into_owned()),
        }])?;
    }

    let args = {
        let mut args = vec![OsStr::new("checkout")];
        if let Some(target) = &target {
            args.push(target);
        }
        args.extend(additional_args.iter().map(OsStr::new));
        args
    };
    let result = git_run_info.run(effects, Some(event_tx_id), args.as_slice())?;

    if result != 0 {
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
        return Ok(result);
    }

    // Determine if we currently have a snapshot checked out, and, if so,
    // attempt to restore it.
    {
        let head_info = repo.get_head_info()?;
        if let Some(head_oid) = head_info.oid {
            let head_commit = repo.find_commit_or_fail(head_oid)?;
            if let Some(snapshot) = WorkingCopySnapshot::try_from_base_commit(repo, &head_commit)? {
                restore_snapshot(repo, &snapshot)?;
            }
        }
    }

    if *render_smartlog {
        let result =
            git_run_info.run_direct_no_wrapping(Some(event_tx_id), &["branchless", "smartlog"])?;
        Ok(result)
    } else {
        Ok(result)
    }
}

fn restore_snapshot(_repo: &Repo, _snapshot: &WorkingCopySnapshot) -> eyre::Result<()> {
    todo!();
}
