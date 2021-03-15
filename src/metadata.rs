//! Additional metadata to display for commits.

//!
//! These are rendered inline in the smartlog, between the commit hash and the
//! commit message.

use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::ops::Add;
use std::time::{Duration, SystemTime};

use console::style;
use fn_error_context::context;
use lazy_static::lazy_static;
use regex::Regex;

use crate::config::{
    get_commit_metadata_branches, get_commit_metadata_differential_revision,
    get_commit_metadata_relative_time,
};

/// Interface to display information about a commit in the smartlog.
pub trait CommitMetadataProvider {
    /// Provide a description of the given commit.
    ///
    /// A return value of `None` indicates that this commit metadata provider was
    /// inapplicable for the provided commit.
    fn describe_commit(&self, commit: &git2::Commit) -> anyhow::Result<Option<String>>;
}

/// Get the complete description for a given commit.
#[context("Rendering commit metadata for commit {:?}", commit.id())]
pub fn render_commit_metadata(
    commit: &git2::Commit,
    commit_metadata_providers: &[&dyn CommitMetadataProvider],
) -> anyhow::Result<String> {
    let descriptions = commit_metadata_providers
        .iter()
        .filter_map(|provider| provider.describe_commit(commit).transpose())
        .collect::<anyhow::Result<Vec<_>>>()?;
    let result = descriptions.join(" ");
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
    #[context("Providing OID metadata for commit {:?}", commit.id())]
    fn describe_commit(&self, commit: &git2::Commit) -> anyhow::Result<Option<String>> {
        let oid = commit.id();
        let oid = &oid.to_string()[..8];
        let oid = if console::user_attended() && self.use_color {
            console::style(oid).yellow().to_string()
        } else {
            oid.to_owned()
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
    #[context("Providing message metadata for commit {:?}", commit.id())]
    fn describe_commit(&self, commit: &git2::Commit) -> anyhow::Result<Option<String>> {
        Ok(commit.summary().map(|summary| summary.to_owned()))
    }
}

/// Display branches that point to a given commit.
pub struct BranchesProvider<'a> {
    is_enabled: bool,
    branch_oid_to_names: &'a HashMap<git2::Oid, HashSet<String>>,
}

impl<'a> BranchesProvider<'a> {
    /// Constructor.
    pub fn new(
        repo: &git2::Repository,
        branch_oid_to_names: &'a HashMap<git2::Oid, HashSet<String>>,
    ) -> anyhow::Result<Self> {
        let is_enabled = get_commit_metadata_branches(repo)?;
        Ok(BranchesProvider {
            is_enabled,
            branch_oid_to_names,
        })
    }
}

impl<'a> CommitMetadataProvider for BranchesProvider<'a> {
    #[context("Providing branch metadata for commit {:?}", commit.id())]
    fn describe_commit(&self, commit: &git2::Commit) -> anyhow::Result<Option<String>> {
        if !self.is_enabled {
            return Ok(None);
        }

        let branch_names: HashSet<&str> = match self.branch_oid_to_names.get(&commit.id()) {
            Some(branch_names) => branch_names
                .iter()
                .map(|branch_name| branch_name.as_ref())
                .collect(),
            None => HashSet::new(),
        };

        if branch_names.is_empty() {
            Ok(None)
        } else {
            let mut branch_names: Vec<&str> = branch_names.into_iter().collect();
            branch_names.sort_unstable();
            let result = style(format!("({})", branch_names.join(", "))).green();
            Ok(Some(result.to_string()))
        }
    }
}

/// Display the associated Phabricator revision for a given commit.
pub struct DifferentialRevisionProvider {
    is_enabled: bool,
}

impl DifferentialRevisionProvider {
    /// Constructor.
    pub fn new(repo: &git2::Repository) -> anyhow::Result<Self> {
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
    let captures = RE.captures(message)?;
    let diff_number = &captures["diff"];

    // HACK: not sure why the matched string doesn't live as long as `message`;
    // we should be able to return it directly here.
    Some(diff_number.to_owned())
}

impl CommitMetadataProvider for DifferentialRevisionProvider {
    #[context("Providing Differential revision metadata for commit {:?}", commit.id())]
    fn describe_commit(&self, commit: &git2::Commit) -> anyhow::Result<Option<String>> {
        if !self.is_enabled {
            return Ok(None);
        }

        let message = match commit.message() {
            Some(message) => message,
            None => return Ok(None),
        };
        let diff_number = match extract_diff_number(message) {
            Some(diff_number) => diff_number,
            None => return Ok(None),
        };
        let result = style(diff_number).green().to_string();
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
    pub fn new(repo: &git2::Repository, now: SystemTime) -> anyhow::Result<Self> {
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
    #[context("Providing relative time metadata for commit {:?}", commit.id())]
    fn describe_commit(&self, commit: &git2::Commit) -> anyhow::Result<Option<String>> {
        if !self.is_enabled {
            return Ok(None);
        }

        let previous_time =
            SystemTime::UNIX_EPOCH.add(Duration::from_secs(commit.time().seconds().try_into()?));
        let description = Self::describe_time_delta(self.now, previous_time)?;
        let result = style(description).green().to_string();
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
