//! Saving and loading of on-disk information for the `git branchless test`
//! subcommand. This isn't part of Git itself, but multiple `git-branchless`
//! subsystems need to know about it, similar to snapshotting.
//!
//! Regrettably, this adds `serde` as a new dependency to `git-branchless-lib`,
//! which will increase build times.

use std::path::PathBuf;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use super::{Commit, NonZeroOid, Repo};

/// The exit status to use when a test command succeeds.
pub const TEST_SUCCESS_EXIT_CODE: i32 = 0;

/// The exit status to use when a test command intends to skip the provided commit.
/// This exit code is used officially by several source control systems:
///
/// - Git: "Note that the script (my_script in the above example) should exit
/// with code 0 if the current source code is good/old, and exit with a code
/// between 1 and 127 (inclusive), except 125, if the current source code is
/// bad/new."
/// - Mercurial: "The exit status of the command will be used to mark revisions
/// as good or bad: status 0 means good, 125 means to skip the revision, 127
/// (command not found) will abort the bisection, and any other non-zero exit
/// status means the revision is bad."
///
/// And it's become the de-facto standard for custom bisection scripts for other
/// source control systems as well.
pub const TEST_INDETERMINATE_EXIT_CODE: i32 = 125;

/// Similarly to `INDETERMINATE_EXIT_CODE`, this exit code is used officially by
/// `git-bisect` and others to abort the process. It's also typically raised by
/// the shell when the command is not found, so it's technically ambiguous
/// whether the command existed or not. Nonetheless, it's intuitive for a
/// failure to run a given command to abort the process altogether, so it
/// shouldn't be too confusing in practice.
pub const TEST_ABORT_EXIT_CODE: i32 = 127;

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

/// Get the path to the file where the latest test command is stored.
pub fn get_latest_test_command_path(repo: &Repo) -> PathBuf {
    get_test_dir(repo).join("latest-command")
}
