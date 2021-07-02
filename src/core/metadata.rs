//! Additional metadata to display for commits.

//!
//! These are rendered inline in the smartlog, between the commit hash and the
//! commit message.

use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::ffi::{OsStr, OsString};
use std::ops::Add;
use std::time::{Duration, SystemTime};

use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;
use fn_error_context::context;
use lazy_static::lazy_static;
use regex::Regex;

use crate::core::config::{
    get_commit_metadata_branches, get_commit_metadata_differential_revision,
    get_commit_metadata_relative_time,
};
use crate::git::{Commit, Repo};

use super::eventlog::{Event, EventCursor, EventReplayer};
use super::formatting::StyledStringBuilder;
use super::graph::CommitGraph;
use super::rewrite::find_rewrite_target;

/// Interface to display information about a commit in the smartlog.
pub trait CommitMetadataProvider {
    /// Provide a description of the given commit.
    ///
    /// A return value of `None` indicates that this commit metadata provider was
    /// inapplicable for the provided commit.
    fn describe_commit(&mut self, commit: &Commit) -> anyhow::Result<Option<StyledString>>;
}

/// Get the complete description for a given commit.
#[context("Rendering commit metadata for commit {:?}", commit.get_oid())]
pub fn render_commit_metadata(
    commit: &Commit,
    commit_metadata_providers: &mut [&mut dyn CommitMetadataProvider],
) -> anyhow::Result<StyledString> {
    let descriptions = commit_metadata_providers
        .iter_mut()
        .filter_map(|provider: &mut &mut dyn CommitMetadataProvider| {
            provider.describe_commit(commit).transpose()
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let result = StyledStringBuilder::join(" ", descriptions);
    Ok(result)
}

/// Display an abbreviated commit hash.
pub struct CommitOidProvider {
    use_color: bool,
}

impl CommitOidProvider {
    /// Constructor.
    pub fn new(use_color: bool) -> anyhow::Result<Self> {
        Ok(CommitOidProvider { use_color })
    }
}

impl CommitMetadataProvider for CommitOidProvider {
    #[context("Providing OID metadata for commit {:?}", commit.get_oid())]
    fn describe_commit(&mut self, commit: &Commit) -> anyhow::Result<Option<StyledString>> {
        let oid = commit.get_oid();
        let oid = &oid.to_string()[..8];
        let oid = if self.use_color {
            StyledString::styled(oid, BaseColor::Yellow.dark())
        } else {
            StyledString::plain(oid)
        };
        Ok(Some(oid))
    }
}

/// Display the first line of the commit message.
pub struct CommitMessageProvider;

impl CommitMessageProvider {
    /// Constructor.
    pub fn new() -> anyhow::Result<Self> {
        Ok(CommitMessageProvider)
    }
}

impl CommitMetadataProvider for CommitMessageProvider {
    #[context("Providing message metadata for commit {:?}", commit.get_oid())]
    fn describe_commit(&mut self, commit: &Commit) -> anyhow::Result<Option<StyledString>> {
        Ok(Some(StyledString::plain(
            commit.get_summary()?.to_string_lossy(),
        )))
    }
}

/// For hidden commits, provide the reason that it's hidden.
pub struct HiddenExplanationProvider<'a> {
    graph: &'a CommitGraph<'a>,
    event_replayer: &'a EventReplayer,
    event_cursor: EventCursor,
}

impl<'a> HiddenExplanationProvider<'a> {
    /// Constructor.
    pub fn new(
        graph: &'a CommitGraph,
        event_replayer: &'a EventReplayer,
        event_cursor: EventCursor,
    ) -> anyhow::Result<Self> {
        Ok(HiddenExplanationProvider {
            graph,
            event_replayer,
            event_cursor,
        })
    }
}

impl<'a> CommitMetadataProvider for HiddenExplanationProvider<'a> {
    fn describe_commit(&mut self, commit: &Commit) -> anyhow::Result<Option<StyledString>> {
        let event = self
            .event_replayer
            .get_cursor_commit_latest_event(self.event_cursor, commit.get_oid());

        let event = match event {
            Some(event) => event,
            None => return Ok(None),
        };

        let result = match event {
            Event::RewriteEvent { .. } => {
                let rewrite_target = find_rewrite_target(
                    self.graph,
                    self.event_replayer,
                    self.event_cursor,
                    commit.get_oid(),
                );
                rewrite_target.map(|rewritten_oid| {
                    StyledString::styled(
                        format!("(rewritten as {})", &rewritten_oid.to_string()[..8]),
                        BaseColor::Black.light(),
                    )
                })
            }

            Event::HideEvent { .. } => Some(StyledString::styled(
                "(manually hidden)",
                BaseColor::Black.light(),
            )),

            Event::RefUpdateEvent { .. }
            | Event::CommitEvent { .. }
            | Event::UnhideEvent { .. } => None,
        };
        Ok(result)
    }
}

/// Display branches that point to a given commit.
pub struct BranchesProvider<'a> {
    is_enabled: bool,
    branch_oid_to_names: &'a HashMap<git2::Oid, HashSet<OsString>>,
}

impl<'a> BranchesProvider<'a> {
    /// Constructor.
    pub fn new(
        repo: &Repo,
        branch_oid_to_names: &'a HashMap<git2::Oid, HashSet<OsString>>,
    ) -> anyhow::Result<Self> {
        let is_enabled = get_commit_metadata_branches(repo)?;
        Ok(BranchesProvider {
            is_enabled,
            branch_oid_to_names,
        })
    }
}

impl<'a> CommitMetadataProvider for BranchesProvider<'a> {
    #[context("Providing branch metadata for commit {:?}", commit.get_oid())]
    fn describe_commit(&mut self, commit: &Commit) -> anyhow::Result<Option<StyledString>> {
        if !self.is_enabled {
            return Ok(None);
        }

        let branch_names: HashSet<&OsStr> = match self.branch_oid_to_names.get(&commit.get_oid()) {
            Some(branch_names) => branch_names
                .iter()
                .map(|branch_name| branch_name.as_os_str())
                .collect(),
            None => HashSet::new(),
        };

        if branch_names.is_empty() {
            Ok(None)
        } else {
            let mut branch_names: Vec<String> = branch_names
                .into_iter()
                .map(|branch_name| branch_name.to_string_lossy().to_string())
                .collect();
            branch_names.sort_unstable();
            let result = StyledString::styled(
                format!("({})", branch_names.join(", ")),
                BaseColor::Green.light(),
            );
            Ok(Some(result))
        }
    }
}

/// Display the associated Phabricator revision for a given commit.
pub struct DifferentialRevisionProvider {
    is_enabled: bool,
}

impl DifferentialRevisionProvider {
    /// Constructor.
    pub fn new(repo: &Repo) -> anyhow::Result<Self> {
        let is_enabled = get_commit_metadata_differential_revision(repo)?;
        Ok(DifferentialRevisionProvider { is_enabled })
    }
}

fn extract_diff_number(message: &str) -> Option<String> {
    lazy_static! {
        static ref RE: Regex = Regex::new(
            r"(?mx)
^
Differential[\ ]Revision:[\ ]
    (.+ /)?
    (?P<diff>D[0-9]+)
$",
        )
        .expect("Failed to compile DifferentialRevisionProvider regex");
    }
    let captures = RE.captures(&message)?;
    let diff_number = &captures["diff"];
    Some(diff_number.to_owned())
}

impl CommitMetadataProvider for DifferentialRevisionProvider {
    #[context("Providing Differential revision metadata for commit {:?}", commit.get_oid())]
    fn describe_commit(&mut self, commit: &Commit) -> anyhow::Result<Option<StyledString>> {
        if !self.is_enabled {
            return Ok(None);
        }

        let diff_number = match extract_diff_number(&commit.get_message_raw()?.to_string_lossy()) {
            Some(diff_number) => diff_number,
            None => return Ok(None),
        };
        let result = StyledString::styled(diff_number, BaseColor::Green.dark());
        Ok(Some(result))
    }
}

/// Display how long ago the given commit was committed.
pub struct RelativeTimeProvider {
    is_enabled: bool,
    now: SystemTime,
}

impl RelativeTimeProvider {
    /// Constructor.
    pub fn new(repo: &Repo, now: SystemTime) -> anyhow::Result<Self> {
        let is_enabled = get_commit_metadata_relative_time(repo)?;
        Ok(RelativeTimeProvider { is_enabled, now })
    }

    /// Whether or not relative times should be shown, according to the user's
    /// settings.
    pub fn is_enabled(&self) -> bool {
        self.is_enabled
    }

    /// Describe a relative time delta, e.g. "3d ago".
    pub fn describe_time_delta(
        now: SystemTime,
        previous_time: SystemTime,
    ) -> anyhow::Result<String> {
        let mut delta: i64 = if previous_time < now {
            let delta = now.duration_since(previous_time)?;
            delta.as_secs().try_into()?
        } else {
            let delta = previous_time.duration_since(now)?;
            -(delta.as_secs().try_into()?)
        };

        if delta < 60 {
            return Ok(format!("{}s", delta));
        }
        delta /= 60;

        if delta < 60 {
            return Ok(format!("{}m", delta));
        }
        delta /= 60;

        if delta < 24 {
            return Ok(format!("{}h", delta));
        }
        delta /= 24;

        if delta < 365 {
            return Ok(format!("{}d", delta));
        }
        delta /= 365;

        // Arguably at this point, users would want a specific date rather than a delta.
        Ok(format!("{}y", delta))
    }
}

impl CommitMetadataProvider for RelativeTimeProvider {
    #[context("Providing relative time metadata for commit {:?}", commit.get_oid())]
    fn describe_commit(&mut self, commit: &Commit) -> anyhow::Result<Option<StyledString>> {
        if !self.is_enabled {
            return Ok(None);
        }

        let previous_time = SystemTime::UNIX_EPOCH
            .add(Duration::from_secs(commit.get_time().seconds().try_into()?));
        let description = Self::describe_time_delta(self.now, previous_time)?;
        let result = StyledString::styled(description, BaseColor::Green.dark());
        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Sub;

    use super::*;

    #[test]
    fn test_extract_diff_number() -> anyhow::Result<()> {
        let message = "\
This is a message

Differential Revision: D123";
        assert_eq!(extract_diff_number(message), Some(String::from("D123")));

        let message = "\
This is a message

Differential Revision: phabricator.com/D123";
        assert_eq!(extract_diff_number(message), Some(String::from("D123")));

        let message = "This is a message";
        assert_eq!(extract_diff_number(message), None);

        Ok(())
    }

    #[test]
    fn test_describe_time_delta() -> anyhow::Result<()> {
        let test_cases: Vec<(isize, &str)> = vec![
            // Could improve formatting for times in the past.
            (-100000, "-100000s"),
            (-1, "-1s"),
            (0, "0s"),
            (10, "10s"),
            (60, "1m"),
            (90, "1m"),
            (120, "2m"),
            (135, "2m"),
            (60 * 45, "45m"),
            (60 * 60 - 1, "59m"),
            (60 * 60, "1h"),
            (60 * 60 * 24 * 3, "3d"),
            (60 * 60 * 24 * 300, "300d"),
            (60 * 60 * 24 * 400, "1y"),
        ];

        for (delta, expected) in test_cases {
            let now = SystemTime::now();
            let previous_time = if delta < 0 {
                let delta = -delta;
                now.add(Duration::from_secs(delta.try_into()?))
            } else {
                now.sub(Duration::from_secs(delta.try_into()?))
            };
            let delta = RelativeTimeProvider::describe_time_delta(now, previous_time)?;
            assert_eq!(delta, expected);
        }

        Ok(())
    }
}
