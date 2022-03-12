//! Additional description metadata to display for commits.
//!
//! These are rendered inline in the smartlog, between the commit hash and the
//! commit message.

use std::borrow::Cow;
use std::collections::HashSet;
use std::convert::TryInto;
use std::ffi::OsStr;
use std::ops::Add;
use std::time::{Duration, SystemTime};

use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;
use lazy_static::lazy_static;
use regex::Regex;
use tracing::instrument;

use crate::core::config::{
    get_commit_descriptors_branches, get_commit_descriptors_differential_revision,
    get_commit_descriptors_relative_time,
};
use crate::git::{
    CategorizedReferenceName, Commit, NonZeroOid, Repo, RepoReferencesSnapshot,
    ResolvedReferenceInfo,
};

use super::eventlog::{Event, EventCursor, EventReplayer};
use super::formatting::{Glyphs, StyledStringBuilder};
use super::rewrite::find_rewrite_target;

/// An object which can be rendered in the smartlog.
#[derive(Clone, Debug)]
pub enum NodeObject<'repo> {
    /// A commit.
    Commit {
        /// The commit.
        commit: Commit<'repo>,
    },

    /// A commit which has been garbage collected, for which detailed
    /// information is no longer available.
    GarbageCollected {
        /// The OID of the garbage-collected commit.
        oid: NonZeroOid,
    },
}

impl<'repo> NodeObject<'repo> {
    fn get_oid(&self) -> NonZeroOid {
        match self {
            NodeObject::Commit { commit } => commit.get_oid(),
            NodeObject::GarbageCollected { oid } => *oid,
        }
    }
}

/// Interface to display information about a node in the smartlog.
pub trait NodeDescriptor {
    /// Provide a description of the given commit.
    ///
    /// A return value of `None` indicates that this commit descriptor was
    /// inapplicable for the provided commit.
    fn describe_node(
        &mut self,
        glyphs: &Glyphs,
        object: &NodeObject,
    ) -> eyre::Result<Option<StyledString>>;
}

/// Get the complete description for a given commit.
#[instrument(skip(node_descriptors))]
pub fn render_node_descriptors(
    glyphs: &Glyphs,
    object: &NodeObject,
    node_descriptors: &mut [&mut dyn NodeDescriptor],
) -> eyre::Result<StyledString> {
    let descriptions = node_descriptors
        .iter_mut()
        .filter_map(|provider: &mut &mut dyn NodeDescriptor| {
            provider.describe_node(glyphs, object).transpose()
        })
        .collect::<eyre::Result<Vec<_>>>()?;
    let result = StyledStringBuilder::join(" ", descriptions);
    Ok(result)
}

/// Display an abbreviated commit hash.
#[derive(Debug)]
pub struct CommitOidDescriptor {
    use_color: bool,
}

impl CommitOidDescriptor {
    /// Constructor.
    pub fn new(use_color: bool) -> eyre::Result<Self> {
        Ok(CommitOidDescriptor { use_color })
    }
}

impl NodeDescriptor for CommitOidDescriptor {
    #[instrument]
    fn describe_node(
        &mut self,
        _glyphs: &Glyphs,
        object: &NodeObject,
    ) -> eyre::Result<Option<StyledString>> {
        let oid = object.get_oid();
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
#[derive(Debug)]
pub struct CommitMessageDescriptor;

impl CommitMessageDescriptor {
    /// Constructor.
    pub fn new() -> eyre::Result<Self> {
        Ok(CommitMessageDescriptor)
    }
}

impl NodeDescriptor for CommitMessageDescriptor {
    #[instrument]
    fn describe_node(
        &mut self,
        _glyphs: &Glyphs,
        object: &NodeObject,
    ) -> eyre::Result<Option<StyledString>> {
        let message = match object {
            NodeObject::Commit { commit } => commit.get_summary()?.to_string_lossy().into_owned(),
            NodeObject::GarbageCollected { oid: _ } => "<garbage collected>".to_string(),
        };
        Ok(Some(StyledString::plain(message)))
    }
}

/// For obsolete commits, provide the reason that it's obsolete.
pub struct ObsolescenceExplanationDescriptor<'a> {
    event_replayer: &'a EventReplayer,
    event_cursor: EventCursor,
}

impl<'a> ObsolescenceExplanationDescriptor<'a> {
    /// Constructor.
    pub fn new(event_replayer: &'a EventReplayer, event_cursor: EventCursor) -> eyre::Result<Self> {
        Ok(ObsolescenceExplanationDescriptor {
            event_replayer,
            event_cursor,
        })
    }
}

impl<'a> NodeDescriptor for ObsolescenceExplanationDescriptor<'a> {
    fn describe_node(
        &mut self,
        _glyphs: &Glyphs,
        object: &NodeObject,
    ) -> eyre::Result<Option<StyledString>> {
        let event = self
            .event_replayer
            .get_cursor_commit_latest_event(self.event_cursor, object.get_oid());

        let event = match event {
            Some(event) => event,
            None => return Ok(None),
        };

        let result = match event {
            Event::RewriteEvent { .. } => {
                let rewrite_target =
                    find_rewrite_target(self.event_replayer, self.event_cursor, object.get_oid());
                rewrite_target.map(|rewritten_oid| {
                    StyledString::styled(
                        format!("(rewritten as {})", &rewritten_oid.to_string()[..8]),
                        BaseColor::Black.light(),
                    )
                })
            }

            Event::ObsoleteEvent { .. } => Some(StyledString::styled(
                "(manually hidden)",
                BaseColor::Black.light(),
            )),

            Event::RefUpdateEvent { .. }
            | Event::CommitEvent { .. }
            | Event::UnobsoleteEvent { .. } => None,
        };
        Ok(result)
    }
}

/// Display branches that point to a given commit.
#[derive(Debug)]
pub struct BranchesDescriptor<'a> {
    is_enabled: bool,
    head_info: &'a ResolvedReferenceInfo<'a>,
    references_snapshot: &'a RepoReferencesSnapshot,
}

impl<'a> BranchesDescriptor<'a> {
    /// Constructor.
    pub fn new(
        repo: &Repo,
        head_info: &'a ResolvedReferenceInfo,
        references_snapshot: &'a RepoReferencesSnapshot,
    ) -> eyre::Result<Self> {
        let is_enabled = get_commit_descriptors_branches(repo)?;
        Ok(BranchesDescriptor {
            is_enabled,
            head_info,
            references_snapshot,
        })
    }
}

impl<'a> NodeDescriptor for BranchesDescriptor<'a> {
    #[instrument]
    fn describe_node(
        &mut self,
        glyphs: &Glyphs,
        object: &NodeObject,
    ) -> eyre::Result<Option<StyledString>> {
        if !self.is_enabled {
            return Ok(None);
        }

        let branch_names: HashSet<&OsStr> = match self
            .references_snapshot
            .branch_oid_to_names
            .get(&object.get_oid())
        {
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
                .map(|branch_name| {
                    let is_checked_out_branch =
                        self.head_info.reference_name == Some(Cow::Borrowed(branch_name));
                    let icon = if is_checked_out_branch {
                        format!(
                            "{} ",
                            glyphs.cycle_arrow // @nocommit rename
                        )
                    } else {
                        "".to_string()
                    };

                    match CategorizedReferenceName::new(branch_name) {
                        reference_name @ CategorizedReferenceName::LocalBranch { .. } => {
                            format!("{}{}", icon, reference_name.render_suffix())
                        }
                        reference_name @ CategorizedReferenceName::RemoteBranch { .. } => {
                            format!("{}remote {}", icon, reference_name.render_suffix())
                        }
                        reference_name @ CategorizedReferenceName::OtherRef { .. } => {
                            format!("{}ref {}", icon, reference_name.render_suffix())
                        }
                    }
                })
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
#[derive(Debug)]
pub struct DifferentialRevisionDescriptor {
    is_enabled: bool,
}

impl DifferentialRevisionDescriptor {
    /// Constructor.
    pub fn new(repo: &Repo) -> eyre::Result<Self> {
        let is_enabled = get_commit_descriptors_differential_revision(repo)?;
        Ok(DifferentialRevisionDescriptor { is_enabled })
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
        .expect("Failed to compile `extract_diff_number` regex");
    }
    let captures = RE.captures(message)?;
    let diff_number = &captures["diff"];
    Some(diff_number.to_owned())
}

impl NodeDescriptor for DifferentialRevisionDescriptor {
    #[instrument]
    fn describe_node(
        &mut self,
        _glyphs: &Glyphs,
        object: &NodeObject,
    ) -> eyre::Result<Option<StyledString>> {
        if !self.is_enabled {
            return Ok(None);
        }
        let commit = match object {
            NodeObject::Commit { commit } => commit,
            NodeObject::GarbageCollected { oid: _ } => return Ok(None),
        };

        let diff_number = match extract_diff_number(&commit.get_message_raw()?.to_string_lossy()) {
            Some(diff_number) => diff_number,
            None => return Ok(None),
        };
        let result = StyledString::styled(diff_number, BaseColor::Green.dark());
        Ok(Some(result))
    }
}

/// Display how long ago the given commit was committed.
#[derive(Debug)]
pub struct RelativeTimeDescriptor {
    is_enabled: bool,
    now: SystemTime,
}

impl RelativeTimeDescriptor {
    /// Constructor.
    pub fn new(repo: &Repo, now: SystemTime) -> eyre::Result<Self> {
        let is_enabled = get_commit_descriptors_relative_time(repo)?;
        Ok(RelativeTimeDescriptor { is_enabled, now })
    }

    /// Whether or not relative times should be shown, according to the user's
    /// settings.
    pub fn is_enabled(&self) -> bool {
        self.is_enabled
    }

    /// Describe a relative time delta, e.g. "3d ago".
    pub fn describe_time_delta(now: SystemTime, previous_time: SystemTime) -> eyre::Result<String> {
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

impl NodeDescriptor for RelativeTimeDescriptor {
    #[instrument]
    fn describe_node(
        &mut self,
        _glyphs: &Glyphs,
        object: &NodeObject,
    ) -> eyre::Result<Option<StyledString>> {
        if !self.is_enabled {
            return Ok(None);
        }
        let commit = match object {
            NodeObject::Commit { commit } => commit,
            NodeObject::GarbageCollected { oid: _ } => return Ok(None),
        };

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
    fn test_extract_diff_number() -> eyre::Result<()> {
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
    fn test_describe_time_delta() -> eyre::Result<()> {
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
            let delta = RelativeTimeDescriptor::describe_time_delta(now, previous_time)?;
            assert_eq!(delta, expected);
        }

        Ok(())
    }
}
