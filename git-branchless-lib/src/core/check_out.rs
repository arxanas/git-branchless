//! Handle checking out commits on disk.

use std::ffi::{OsStr, OsString};
use std::fmt::Write;

use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;
use tracing::instrument;

use crate::git::{CategorizedReferenceName, GitRunInfo, Repo};

use super::effects::Effects;
use super::eventlog::{EventLogDb, EventTransactionId};
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
    _repo: &Repo,
    _event_log_db: &EventLogDb,
    event_tx_id: Option<EventTransactionId>,
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

    let args = {
        let mut args = vec![OsStr::new("checkout")];
        if let Some(target) = &target {
            args.push(target);
        }
        args.extend(additional_args.iter().map(OsStr::new));
        args
    };
    let result = git_run_info.run(effects, event_tx_id, args.as_slice())?;

    if result == 0 {
        if *render_smartlog {
            let result =
                git_run_info.run_direct_no_wrapping(event_tx_id, &["branchless", "smartlog"])?;
            Ok(result)
        } else {
            Ok(result)
        }
    } else {
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
        Ok(result)
    }
}
