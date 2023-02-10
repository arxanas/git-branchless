//! Saving and loading of on-disk information for the `git branchless test`
//! subcommand. This isn't part of Git itself, but multiple `git-branchless`
//! subsystems need to know about it, similar to snapshotting.
//!
//! Regrettably, this adds `serde` as a new dependency to `git-branchless-lib`,
//! which will increase build times.

use std::path::PathBuf;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use super::{Commit, NonZeroOid, Repo};

/// Convert a command string into a string that's safe to use as a filename.
pub fn make_test_command_slug(command: String) -> String {
    command.replace(['/', ' ', '\n'], "__")
}

/// A version of `NonZeroOid` that can be serialized and deserialized. This
/// exists in case we want to move this type (back) into a separate module which
/// has a `serde` dependency in the interest of improving build times.
#[derive(Debug)]
pub struct SerializedNonZeroOid(pub NonZeroOid);

impl Serialize for SerializedNonZeroOid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for SerializedNonZeroOid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let oid: NonZeroOid = s.parse().map_err(|_| {
            de::Error::invalid_value(de::Unexpected::Str(&s), &"a valid non-zero OID")
        })?;
        Ok(SerializedNonZeroOid(oid))
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(missing_docs)]
pub struct SerializedTestResult {
    pub command: String,
    pub exit_code: i32,
    pub fixed_tree_oid: Option<SerializedNonZeroOid>,
    #[serde(default)]
    pub interactive: bool,
}

/// Get the directory where the results of running tests are stored.
fn get_test_dir(repo: &Repo) -> PathBuf {
    repo.get_path().join("branchless").join("test")
}

/// Get the directory where the result of tests for a particular commit are
/// stored. Tests are keyed by tree OID, not commit OID, so that they can be
/// cached based on the contents of the commit, rather than its specific commit
/// hash. This means that we can cache the results of tests for commits that
/// have been amended or rebased.
pub fn get_test_tree_dir(repo: &Repo, commit: &Commit) -> PathBuf {
    get_test_dir(repo).join(commit.get_tree_oid().to_string())
}

/// Get the directory where the locks for running tests are stored.
pub fn get_test_locks_dir(repo: &Repo) -> PathBuf {
    get_test_dir(repo).join("locks")
}

/// Get the directory where the worktrees for running tests are stored.
pub fn get_test_worktrees_dir(repo: &Repo) -> PathBuf {
    get_test_dir(repo).join("worktrees")
}
