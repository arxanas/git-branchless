//! Operations on the Git repository. This module exists for a few reasons:
//!
//! - To ensure that every call to a Git operation has an associated `wrap_err`
//! for use with `Try`.
//! - To improve the interface in some cases. In particular, some operations in
//! `git2` return an `Error` with code `ENOTFOUND`, but we should really return
//! an `Option` in those cases.
//! - To make it possible to audit all the Git operations carried out in the
//! codebase.
//! - To collect some different helper Git functions.

use std::borrow::{Borrow, Cow};
use std::collections::{HashMap, HashSet};
use std::convert::{TryFrom, TryInto};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::SystemTime;

use chrono::{DateTime, Local, TimeZone, Utc};
use color_eyre::Help;
use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;
use eyre::{eyre, Context};
use itertools::Itertools;
use lazy_static::lazy_static;
use os_str_bytes::{OsStrBytes, OsStringBytes};
use regex::bytes::Regex;
use tracing::{instrument, warn};

use crate::core::config::get_main_branch_name;
use crate::core::effects::{Effects, OperationType};
use crate::core::eventlog::EventTransactionId;
use crate::core::formatting::StyledStringBuilder;
use crate::core::node_descriptors::{
    render_node_descriptors, CommitMessageDescriptor, CommitOidDescriptor, NodeObject,
};
use crate::git::config::{Config, ConfigRead};
use crate::git::oid::{make_non_zero_oid, MaybeZeroOid, NonZeroOid};
use crate::git::run::GitRunInfo;
use crate::git::tree::{dehydrate_tree, get_changed_paths_between_trees, hydrate_tree, Tree};

/// Convert a `git2::Error` into an `eyre::Error` with an auto-generated message.
pub(super) fn wrap_git_error(error: git2::Error) -> eyre::Error {
    eyre::eyre!("Git error {:?}: {}", error.code(), error.message())
}
/// A snapshot of information about a certain reference. Updates to the
/// reference after this value is obtained are not reflected.
///
/// `HEAD` is typically a symbolic reference, which means that it's a reference
/// that points to another reference. Usually, the other reference is a branch.
/// In this way, you can check out a branch and move the branch (e.g. by
/// committing) and `HEAD` is also effectively updated (you can traverse the
/// pointed-to reference and get the current commit OID).
///
/// There are a couple of interesting edge cases to worry about:
///
/// - `HEAD` is detached. This means that it's pointing directly to a commit and
/// is not a symbolic reference for the time being. This is uncommon in normal
/// Git usage, but very common in `git-branchless` usage.
/// - `HEAD` is unborn. This means that it doesn't even exist yet. This happens
/// when a repository has been freshly initialized, but no commits have been
/// made, for example.
#[derive(Debug, PartialEq, Eq)]
pub struct ResolvedReferenceInfo<'a> {
    /// The OID of the commit that `HEAD` points to. If `HEAD` is unborn, then
    /// this is `None`.
    pub oid: Option<NonZeroOid>,

    /// The name of the reference that `HEAD` points to symbolically. If `HEAD`
    /// is detached, then this is `None`.
    pub reference_name: Option<Cow<'a, OsStr>>,
}

impl<'a> ResolvedReferenceInfo<'a> {
    /// Get the name of the branch, if any. Returns `None` if `HEAD` is
    /// detached.  The `refs/heads/` prefix, if any, is stripped.
    pub fn get_branch_name(&self) -> eyre::Result<Option<OsString>> {
        let reference_name: &OsStr = match &self.reference_name {
            Some(reference_name) => reference_name,
            None => return Ok(None),
        };

        let reference_name_bytes = reference_name.to_raw_bytes();
        match reference_name_bytes.strip_prefix(b"refs/heads/") {
            None => Ok(Some(reference_name.to_owned())),
            Some(branch_name) => {
                let branch_name = OsStringBytes::from_raw_vec(branch_name.to_vec())?;
                Ok(Some(branch_name))
            }
        }
    }
}

/// The parsed version of Git.
#[derive(Debug, PartialEq, PartialOrd, Eq)]
pub struct GitVersion(pub isize, pub isize, pub isize);

impl FromStr for GitVersion {
    type Err = eyre::Error;

    #[instrument]
    fn from_str(output: &str) -> eyre::Result<GitVersion> {
        let output = output.trim();
        let words = output.split(&[' ', '-'][..]).collect::<Vec<&str>>();
        let version_str = match &words.as_slice() {
            [_git, _version, version_str, ..] => version_str,
            _ => eyre::bail!("Could not parse Git version output: {:?}", output),
        };
        match version_str.split('.').collect::<Vec<&str>>().as_slice() {
            [major, minor, patch, ..] => {
                let major = major.parse()?;
                let minor = minor.parse()?;

                // Example version without a real patch number: `2.33.GIT`.
                let patch: isize = patch.parse().unwrap_or_default();

                Ok(GitVersion(major, minor, patch))
            }
            _ => eyre::bail!("Could not parse Git version string: {}", version_str),
        }
    }
}

/// Options for `Repo::cherry_pick_fast`.
#[derive(Clone, Debug)]
pub struct CherryPickFastOptions {
    /// Detect if a commit is being applied onto a parent with the same tree,
    /// and skip applying the patch in that case.
    pub reuse_parent_tree_if_possible: bool,
}

/// An error raised when attempting the `Repo::cherry_pick_fast` operation.
#[derive(Debug)]
pub enum CherryPickFastError {
    /// A merge conflict occurred, so the cherry-pick could not continue.
    MergeConflict {
        /// The paths that were in conflict.
        conflicting_paths: HashSet<PathBuf>,
    },
}

/// Options for `Repo::amend_fast`
#[derive(Debug)]
pub enum AmendFastOptions {
    /// Amend a set of paths from the current state of the working copy.
    FromWorkingCopy {
        /// The status entries for the files to amend.
        status_entries: Vec<StatusEntry>,
    },
    /// Amend a set of paths from the current state of the index.
    FromIndex {
        /// The paths to amend.
        paths: Vec<PathBuf>,
    },
}

impl AmendFastOptions {
    /// Returns whether there are any paths to be amended.
    pub fn is_empty(&self) -> bool {
        match &self {
            AmendFastOptions::FromIndex { paths } => paths.is_empty(),
            AmendFastOptions::FromWorkingCopy { status_entries } => status_entries.is_empty(),
        }
    }
}

/// A snapshot of all the positions of references we care about in the repository.
#[derive(Debug)]
pub struct RepoReferencesSnapshot {
    /// The location of the `HEAD` reference. This may be `None` if `HEAD` is unborn.
    pub head_oid: Option<NonZeroOid>,

    /// The location of the main branch.
    pub main_branch_oid: NonZeroOid,

    /// A mapping from commit OID to the branches which point to that commit.
    pub branch_oid_to_names: HashMap<NonZeroOid, HashSet<OsString>>,
}

/// Wrapper around `git2::Repository`.
pub struct Repo {
    pub(super) inner: git2::Repository,
}

impl std::fmt::Debug for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<Git repository at: {:?}>", self.get_path())
    }
}

impl Repo {
    /// Get the Git repository associated with the given directory.
    #[instrument]
    pub fn from_dir(path: &Path) -> eyre::Result<Self> {
        let repo = git2::Repository::discover(path).map_err(wrap_git_error)?;
        Ok(Repo { inner: repo })
    }

    /// Get the Git repository associated with the current directory.
    #[instrument]
    pub fn from_current_dir() -> eyre::Result<Self> {
        let path = std::env::current_dir().wrap_err("Getting working directory")?;
        Repo::from_dir(&path)
    }

    /// Open a new copy of the repository.
    pub fn try_clone(&self) -> eyre::Result<Self> {
        let path = self.get_path();
        let repo = git2::Repository::open(path)?;
        Ok(Repo { inner: repo })
    }

    /// Get the path to the `.git` directory for the repository.
    pub fn get_path(&self) -> &Path {
        self.inner.path()
    }

    /// Get the path to the `packed-refs` file for the repository.
    pub fn get_packed_refs_path(&self) -> PathBuf {
        self.inner.path().join("packed-refs")
    }

    /// Get the path to the directory inside the `.git` directory which contains
    /// state used for the current rebase (if any).
    pub fn get_rebase_state_dir_path(&self) -> PathBuf {
        self.inner.path().join("rebase-merge")
    }

    /// Get the path to the working copy for this repository. If the repository
    /// is bare (has no working copy), returns `None`.
    pub fn get_working_copy_path(&self) -> Option<&Path> {
        self.inner.workdir()
    }

    /// Get the index file for this repository.
    pub fn get_index(&self) -> eyre::Result<Index> {
        Ok(Index {
            inner: self.inner.index()?,
        })
    }

    /// Get the configuration object for the repository.
    ///
    /// **Warning**: This object should only be used for read operations. Write
    /// operations should go to the `config` file under the `.git/branchless`
    /// directory.
    #[instrument]
    pub fn get_readonly_config(&self) -> eyre::Result<impl ConfigRead> {
        let config = self
            .inner
            .config()
            .map_err(wrap_git_error)
            .wrap_err("Creating `git2::Config` object")?;
        Ok(Config::from(config))
    }

    /// Get the file where git-branchless-specific Git configuration is stored.
    #[instrument]
    pub fn get_config_path(&self) -> PathBuf {
        self.get_path().join("branchless").join("config")
    }

    /// Get the directory where the DAG for the repository is stored.
    #[instrument]
    pub fn get_dag_dir(&self) -> PathBuf {
        self.get_path().join("branchless").join("dag")
    }

    /// Get the directory to store man-pages. Note that this is the `man`
    /// directory, and not a subsection thereof. `git-branchless` man-pages must
    /// go into the `man/man1` directory to be found by `man`.
    #[instrument]
    pub fn get_man_dir(&self) -> PathBuf {
        self.get_path().join("branchless").join("man")
    }

    /// Get a directory suitable for storing temporary files.
    ///
    /// In particular, this directory is guaranteed to be on the same filesystem
    /// as the Git repository itself, so you can move files between them
    /// atomically. See
    /// <https://github.com/arxanas/git-branchless/discussions/120>.
    #[instrument]
    pub fn get_tempfile_dir(&self) -> PathBuf {
        self.get_path().join("branchless").join("tmp")
    }

    /// Get the connection to the SQLite database for this repository.
    #[instrument]
    pub fn get_db_conn(&self) -> eyre::Result<rusqlite::Connection> {
        let dir = self.get_path().join("branchless");
        std::fs::create_dir_all(&dir).wrap_err("Creating .git/branchless dir")?;
        let path = dir.join("db.sqlite3");
        let conn = rusqlite::Connection::open(&path)
            .wrap_err_with(|| format!("Opening database connection at {:?}", &path))?;
        Ok(conn)
    }

    /// Get a snapshot of information about a given reference.
    #[instrument]
    pub fn resolve_reference(&self, reference: &Reference) -> eyre::Result<ResolvedReferenceInfo> {
        let oid = reference.peel_to_commit()?.map(|commit| commit.get_oid());
        let reference_name: Option<OsString> = match reference.inner.kind() {
            Some(git2::ReferenceType::Direct) => None,
            Some(git2::ReferenceType::Symbolic) => match reference.inner.symbolic_target_bytes() {
                Some(name) => Some(OsStringBytes::from_raw_vec(name.to_vec())?),
                None => eyre::bail!(
                    "Reference was resolved to OID: {:?}, but its name could not be decoded: {:?}",
                    oid,
                    reference.inner.name_bytes()
                ),
            },
            None => eyre::bail!("Unknown `HEAD` reference type"),
        };
        Ok(ResolvedReferenceInfo {
            oid,
            reference_name: reference_name.map(Cow::Owned),
        })
    }

    /// Get the OID for the repository's `HEAD` reference.
    #[instrument]
    pub fn get_head_info(&self) -> eyre::Result<ResolvedReferenceInfo> {
        match self.find_reference(OsStr::new("HEAD"))? {
            Some(reference) => self.resolve_reference(&reference),
            None => Ok(ResolvedReferenceInfo {
                oid: None,
                reference_name: None,
            }),
        }
    }

    /// Set the `HEAD` reference directly to the provided `oid`. Does not touch
    /// the working copy.
    #[instrument]
    pub fn set_head(&self, oid: NonZeroOid) -> eyre::Result<()> {
        self.inner.set_head_detached(oid.inner)?;
        Ok(())
    }

    /// Detach `HEAD` by making it point directly to its current OID, rather
    /// than to a branch. If `HEAD` is already detached, logs a warning.
    #[instrument]
    pub fn detach_head(&self, head_info: &ResolvedReferenceInfo) -> eyre::Result<()> {
        match head_info.oid {
            Some(oid) => self
                .inner
                .set_head_detached(oid.inner)
                .map_err(wrap_git_error),
            None => {
                warn!("Attempted to detach `HEAD` while `HEAD` is unborn");
                Ok(())
            }
        }
    }

    /// Get the `Reference` for the main branch for the repository.
    pub fn get_main_branch_reference(&self) -> eyre::Result<Reference> {
        let main_branch_name = get_main_branch_name(self)?;
        match self.find_branch(&main_branch_name, git2::BranchType::Local)? {
            Some(branch) => {
                let upstream_branch = branch
                    .inner
                    .upstream()
                    .map(|branch| Branch { inner: branch })
                    .unwrap_or_else(|_| branch);
                Ok(upstream_branch.into_reference())
            }
            None => match self.find_branch(&main_branch_name, git2::BranchType::Remote)? {
                Some(branch) => Ok(branch.into_reference()),
                None => {
                    let suggestion = format!(
                        r"
The main branch {:?} could not be found in your repository
at path: {:?}.
These branches exist: {:?}
Either create it, or update the main branch setting by running:

    git config branchless.core.mainBranch <branch>
",
                        get_main_branch_name(self)?,
                        self.get_path(),
                        self.get_all_local_branches()?
                            .into_iter()
                            .map(|branch| {
                                branch
                                    .into_reference()
                                    .get_name()
                                    .map(|s| format!("{:?}", s))
                            })
                            .collect::<eyre::Result<Vec<String>>>()?,
                    );
                    Err(eyre!("Could not find repository main branch")
                        .with_suggestion(|| suggestion))
                }
            },
        }
    }

    /// Get the OID corresponding to the main branch.
    #[instrument]
    pub fn get_main_branch_oid(&self) -> eyre::Result<NonZeroOid> {
        let main_branch_reference = self.get_main_branch_reference()?;
        let commit = main_branch_reference.peel_to_commit()?;
        match commit {
            Some(commit) => Ok(commit.get_oid()),
            None => eyre::bail!(
                "Could not find commit pointed to by main branch: {:?}",
                main_branch_reference.get_name()?
            ),
        }
    }

    /// Get a mapping from OID to the names of branches which point to that OID.
    ///
    /// The returned branch names include the `refs/heads/` prefix, so it must
    /// be stripped if desired.
    #[instrument]
    pub fn get_branch_oid_to_names(&self) -> eyre::Result<HashMap<NonZeroOid, HashSet<OsString>>> {
        let mut result: HashMap<NonZeroOid, HashSet<OsString>> = HashMap::new();
        for branch in self.get_all_local_branches()? {
            let reference = branch.into_reference();
            let reference_name = reference.get_name()?;
            let reference_info = self.resolve_reference(&reference)?;
            if let Some(reference_oid) = reference_info.oid {
                result
                    .entry(reference_oid)
                    .or_insert_with(HashSet::new)
                    .insert(reference_name);
            }
        }

        // The main branch may be a remote branch, in which case it won't be
        // returned in the iteration above.
        let main_branch_name = self.get_main_branch_reference()?.get_name()?;
        let main_branch_oid = self.get_main_branch_oid()?;
        result
            .entry(main_branch_oid)
            .or_insert_with(HashSet::new)
            .insert(main_branch_name);

        Ok(result)
    }

    /// Get the positions of references in the repository.
    pub fn get_references_snapshot(&self) -> eyre::Result<RepoReferencesSnapshot> {
        let head_oid = self.get_head_info()?.oid;
        let main_branch_oid = self.get_main_branch_oid()?;
        let branch_oid_to_names = self.get_branch_oid_to_names()?;

        Ok(RepoReferencesSnapshot {
            head_oid,
            main_branch_oid,
            branch_oid_to_names,
        })
    }

    /// Detect if an interactive rebase has started but not completed.
    ///
    /// Git will send us spurious `post-rewrite` events marked as `amend` during an
    /// interactive rebase, indicating that some of the commits have been rewritten
    /// as part of the rebase plan, but not all of them. This function attempts to
    /// detect when an interactive rebase is underway, and if the current
    /// `post-rewrite` event is spurious.
    ///
    /// There are two practical issues for users as a result of this Git behavior:
    ///
    ///   * During an interactive rebase, we may see many "processing 1 rewritten
    ///   commit" messages, and then a final "processing X rewritten commits" message
    ///   once the rebase has concluded. This is potentially confusing for users, since
    ///   the operation logically only rewrote the commits once, but we displayed the
    ///   message multiple times.
    ///
    ///   * During an interactive rebase, we may warn about abandoned commits, when the
    ///   next operation in the rebase plan fixes up the abandoned commit. This can
    ///   happen even if no conflict occurred and the rebase completed successfully
    ///   without any user intervention.
    #[instrument]
    pub fn is_rebase_underway(&self) -> eyre::Result<bool> {
        use git2::RepositoryState::*;
        match self.inner.state() {
            Rebase | RebaseInteractive | RebaseMerge => Ok(true),

            // Possibly some of these states should also be treated as `true`?
            Clean | Merge | Revert | RevertSequence | CherryPick | CherryPickSequence | Bisect
            | ApplyMailbox | ApplyMailboxOrRebase => Ok(false),
        }
    }

    /// Get the type current multi-step operation (such as `rebase` or
    /// `cherry-pick`) which is underway. Returns `None` if there is no such
    /// operation.
    pub fn get_current_operation_type(&self) -> Option<&str> {
        use git2::RepositoryState::*;
        match self.inner.state() {
            Clean | Bisect => None,
            Merge => Some("merge"),
            Revert | RevertSequence => Some("revert"),
            CherryPick | CherryPickSequence => Some("cherry-pick"),
            Rebase | RebaseInteractive | RebaseMerge => Some("rebase"),
            ApplyMailbox | ApplyMailboxOrRebase => Some("am"),
        }
    }

    /// Find the merge-base between two commits. Returns `None` if a merge-base
    /// could not be found.
    #[instrument]
    pub fn find_merge_base(
        &self,
        lhs: NonZeroOid,
        rhs: NonZeroOid,
    ) -> eyre::Result<Option<NonZeroOid>> {
        match self.inner.merge_base(lhs.inner, rhs.inner) {
            Ok(merge_base_oid) => Ok(Some(make_non_zero_oid(merge_base_oid))),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => Err(wrap_git_error(err)),
        }
    }

    /// Get the patch for a commit, i.e. the diff between that commit and its
    /// parent.
    ///
    /// If the commit has more than one parent, returns `None`.
    #[instrument]
    pub fn get_patch_for_commit(
        &self,
        effects: &Effects,
        commit: &Commit,
    ) -> eyre::Result<Option<Diff>> {
        let (_effects, _progress) = effects.start_operation(OperationType::CalculateDiff);

        let changed_paths = match self.get_paths_touched_by_commit(commit)? {
            None => return Ok(None),
            Some(changed_paths) => changed_paths,
        };
        let dehydrated_commit = self.dehydrate_commit(
            commit,
            changed_paths
                .iter()
                .map(|x| -> &Path { x })
                .collect_vec()
                .as_slice(),
            true,
        )?;

        let parent = dehydrated_commit.get_only_parent();
        let parent_tree = match &parent {
            Some(parent) => Some(parent.get_tree()?.inner.clone()),
            None => None,
        };
        let current_tree = dehydrated_commit.get_tree()?;

        let diff = self
            .inner
            .diff_tree_to_tree(parent_tree.as_ref(), Some(&current_tree.inner), None)
            .wrap_err_with(|| format!("Calculating diff between: {:?}", commit))?;
        Ok(Some(Diff { inner: diff }))
    }

    /// Returns the set of paths currently staged to the repository's index.
    #[instrument]
    pub fn get_staged_paths(&self) -> eyre::Result<HashSet<PathBuf>> {
        let head_commit_oid = match self.get_head_info()?.oid {
            Some(oid) => oid,
            None => eyre::bail!("No HEAD to check for staged paths"),
        };
        let head_commit = self.find_commit_or_fail(head_commit_oid)?;
        let head_tree = self.find_tree_or_fail(head_commit.get_tree()?.get_oid())?;

        let diff = self.inner.diff_tree_to_index(
            Some(&head_tree.inner),
            Some(&self.get_index()?.inner),
            None,
        )?;
        let paths = diff
            .deltas()
            .into_iter()
            .flat_map(|delta| vec![delta.old_file().path(), delta.new_file().path()])
            .flat_map(|p| p.map(PathBuf::from))
            .collect();
        Ok(paths)
    }

    /// Get the file paths which were added, removed, or changed by the given
    /// commit.
    ///
    /// If the commit has no parents, returns all of the file paths in that
    /// commit's tree.
    ///
    /// If the commit has more than one parent, returns `None`.
    #[instrument]
    pub fn get_paths_touched_by_commit(
        &self,
        commit: &Commit,
    ) -> eyre::Result<Option<HashSet<PathBuf>>> {
        let parent_commits = commit.get_parents();
        let parent_tree = match parent_commits.as_slice() {
            [] => None,
            [only_parent] => Some(only_parent.get_tree()?.inner),
            [..] => return Ok(None),
        };

        let current_tree = commit.get_tree()?.inner;
        let changed_paths =
            get_changed_paths_between_trees(self, parent_tree.as_ref(), Some(&current_tree))?;
        Ok(Some(changed_paths))
    }

    /// Get the patch ID for this commit.
    #[instrument]
    pub fn get_patch_id(
        &self,
        effects: &Effects,
        commit: &Commit,
    ) -> eyre::Result<Option<PatchId>> {
        let patch = match self.get_patch_for_commit(effects, commit)? {
            None => return Ok(None),
            Some(diff) => diff,
        };
        let patch_id = {
            let (_effects, _progress) = effects.start_operation(OperationType::CalculatePatchId);
            patch.inner.patchid(None).wrap_err("Computing patch ID")?
        };
        Ok(Some(PatchId { patch_id }))
    }

    /// Attempt to parse the user-provided object descriptor.
    pub fn revparse_single_commit(&self, spec: &str) -> eyre::Result<Option<Commit>> {
        match self.inner.revparse_single(spec) {
            Ok(object) => match object.into_commit() {
                Ok(commit) => Ok(Some(Commit { inner: commit })),
                Err(_) => Ok(None),
            },
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => Err(wrap_git_error(err)),
        }
    }

    /// Look up a single reference by name.
    pub fn get_reference(&self, reference_name: &OsStr) -> eyre::Result<Option<Reference>> {
        let reference_name = reference_name.to_str().ok_or_else(|| {
            eyre::eyre!(
                "Cannot convert reference name to string (libgit2 limitation): {:?}",
                reference_name
            )
        })?;
        match self.inner.find_reference(reference_name) {
            Ok(reference) => Ok(Some(Reference { inner: reference })),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => (Err(err.into())),
        }
    }

    /// Find all references in the repository.
    #[instrument]
    pub fn get_all_references(&self) -> eyre::Result<Vec<Reference>> {
        let mut all_references = Vec::new();
        for reference in self
            .inner
            .references()
            .map_err(wrap_git_error)
            .wrap_err("Iterating through references")?
        {
            let reference = reference.wrap_err("Accessing individual reference")?;
            all_references.push(Reference { inner: reference });
        }
        Ok(all_references)
    }

    /// Check if the repository has staged or unstaged changes. Untracked files
    /// are not included. This operation may take a while.
    #[instrument]
    pub fn has_changed_files(
        &self,
        effects: &Effects,
        git_run_info: &GitRunInfo,
    ) -> eyre::Result<bool> {
        let exit_code = git_run_info.run(
            effects,
            // This is not a mutating operation, so we don't need a transaction ID.
            None,
            &["diff", "--quiet"],
        )?;
        if exit_code == 0 {
            Ok(false)
        } else {
            Ok(true)
        }
    }

    /// Returns the current status of the repo index and working copy.
    pub fn get_status(
        &self,
        git_run_info: &GitRunInfo,
        event_tx_id: Option<EventTransactionId>,
    ) -> eyre::Result<Vec<StatusEntry>> {
        let output = git_run_info
            .run_silent(
                self,
                event_tx_id,
                &["status", "--porcelain=v2", "--untracked-files=no", "-z"],
                Default::default(),
            )?
            .stdout;

        let not_null_terminator = |c: &u8| *c != 0_u8;
        let mut statuses = Vec::new();
        let mut status_bytes = output.into_iter().peekable();

        // Iterate over the status entries in the output.
        // This takes some care, because NUL bytes are both used to delimit
        // between entries, and as a separator between paths in the case
        // of renames.
        // See https://git-scm.com/docs/git-status#_porcelain_format_version_2
        while let Some(line_prefix) = status_bytes.peek() {
            let line = match line_prefix {
                // Ordinary change entry.
                b'1' => {
                    let line = status_bytes
                        .by_ref()
                        .take_while(not_null_terminator)
                        .collect_vec();
                    line
                }
                // Rename or copy change entry.
                b'2' => {
                    let mut line = status_bytes
                        .by_ref()
                        .take_while(not_null_terminator)
                        .collect_vec();
                    line.push(0_u8); // Persist first null terminator in the line.
                    line.extend(status_bytes.by_ref().take_while(not_null_terminator));
                    line
                }
                _ => eyre::bail!("unknown status line prefix: {}", line_prefix),
            };
            let entry = line.as_slice().try_into()?;
            statuses.push(entry);
        }
        Ok(statuses)
    }

    /// Create a new reference or update an existing one.
    #[instrument]
    pub fn create_reference(
        &self,
        name: &OsStr,
        oid: NonZeroOid,
        force: bool,
        log_message: &str,
    ) -> eyre::Result<Reference> {
        let name = match name.to_str() {
            Some(name) => name,
            None => eyre::bail!(
                "Reference name is not a UTF-8 string (libgit2 limitation): {:?}",
                name
            ),
        };
        let reference = self
            .inner
            .reference(name, oid.inner, force, log_message)
            .map_err(wrap_git_error)?;
        Ok(Reference { inner: reference })
    }

    /// Look up a reference with the given name. Returns `None` if not found.
    #[instrument]
    pub fn find_reference(&self, name: &OsStr) -> eyre::Result<Option<Reference>> {
        let name = match name.to_str() {
            Some(name) => name,
            None => eyre::bail!(
                "Reference name is not a UTF-8 string (libgit2 limitation): {:?}",
                name
            ),
        };
        match self.inner.find_reference(name) {
            Ok(reference) => Ok(Some(Reference { inner: reference })),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => Err(wrap_git_error(err)),
        }
    }

    /// Get all local branches in the repository.
    #[instrument]
    pub fn get_all_local_branches(&self) -> eyre::Result<Vec<Branch>> {
        let mut all_branches = Vec::new();
        for branch in self
            .inner
            .branches(Some(git2::BranchType::Local))
            .map_err(wrap_git_error)
            .wrap_err("Iterating over all local branches")?
        {
            let (branch, _branch_type) = branch.wrap_err("Accessing individual branch")?;
            all_branches.push(Branch { inner: branch });
        }
        Ok(all_branches)
    }

    /// Look up the branch with the given name. Returns `None` if not found.
    #[instrument]
    pub fn find_branch(&self, name: &str, branch_type: BranchType) -> eyre::Result<Option<Branch>> {
        match self.inner.find_branch(name, branch_type) {
            Ok(branch) => Ok(Some(Branch { inner: branch })),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => Err(wrap_git_error(err)),
        }
    }

    /// Create a new branch or update an existing branch.
    #[instrument]
    pub fn create_branch(
        &self,
        name: &OsStr,
        target: &Commit,
        force: bool,
    ) -> eyre::Result<git2::Branch> {
        let name = match name.to_str() {
            Some(name) => name,
            None => eyre::bail!(
                "Branch name is not a UTF-8 string (libgit2 limitation): {:?}",
                name
            ),
        };
        self.inner
            .branch(name, &target.inner, force)
            .map_err(wrap_git_error)
    }

    /// Look up a commit with the given OID. Returns `None` if not found.
    #[instrument]
    pub fn find_commit(&self, oid: NonZeroOid) -> eyre::Result<Option<Commit>> {
        match self.inner.find_commit(oid.inner) {
            Ok(commit) => Ok(Some(Commit { inner: commit })),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => Err(wrap_git_error(err)),
        }
    }

    /// Like `find_commit`, but raises a generic error if the commit could not
    /// be found.
    #[instrument]
    pub fn find_commit_or_fail(&self, oid: NonZeroOid) -> eyre::Result<Commit> {
        match self.find_commit(oid) {
            Ok(Some(commit)) => Ok(commit),
            Ok(None) => eyre::bail!("Could not find commit with OID: {:?}", oid),
            Err(err) => Err(err),
        }
    }

    /// Look up the commit with the given OID and render a friendly description
    /// of it, or render an error message if not found.
    pub fn friendly_describe_commit_from_oid(&self, oid: NonZeroOid) -> eyre::Result<StyledString> {
        match self.find_commit(oid)? {
            Some(commit) => Ok(commit.friendly_describe()?),
            None => {
                let NonZeroOid { inner: oid } = oid;
                Ok(StyledString::styled(
                    format!("<commit not available: {}>", oid),
                    BaseColor::Red.light(),
                ))
            }
        }
    }

    /// Create a new commit.
    #[instrument]
    pub fn create_commit(
        &self,
        update_ref: Option<&str>,
        author: &Signature,
        committer: &Signature,
        message: &str,
        tree: &Tree,
        parents: Vec<&Commit>,
    ) -> eyre::Result<NonZeroOid> {
        let parents = parents
            .iter()
            .map(|commit| &commit.inner)
            .collect::<Vec<_>>();
        let oid = self
            .inner
            .commit(
                update_ref,
                &author.inner,
                &committer.inner,
                message,
                &tree.inner,
                parents.as_slice(),
            )
            .map_err(wrap_git_error)?;
        Ok(make_non_zero_oid(oid))
    }

    /// Cherry-pick a commit in memory and return the resulting index.
    #[instrument]
    pub fn cherry_pick_commit(
        &self,
        cherry_pick_commit: &Commit,
        our_commit: &Commit,
        mainline: u32,
    ) -> eyre::Result<Index> {
        let index = self
            .inner
            .cherrypick_commit(&cherry_pick_commit.inner, &our_commit.inner, mainline, None)
            .map_err(wrap_git_error)?;
        Ok(Index { inner: index })
    }

    /// Cherry-pick a commit in memory and return the resulting tree.
    ///
    /// The `libgit2` routines operate on entire `Index`es, which contain one
    /// entry per file in the repository. When operating on a large repository,
    /// this is prohibitively slow, as it takes several seconds just to write
    /// the index to disk. To improve performance, we reduce the size of the
    /// involved indexes by filtering out any unchanged entries from the input
    /// trees, then call into `libgit2`, then add back the unchanged entries to
    /// the output tree.
    #[instrument]
    pub fn cherry_pick_fast<'repo>(
        &'repo self,
        patch_commit: &'repo Commit,
        target_commit: &'repo Commit,
        options: &CherryPickFastOptions,
    ) -> eyre::Result<Result<Tree<'repo>, CherryPickFastError>> {
        let CherryPickFastOptions {
            reuse_parent_tree_if_possible,
        } = options;

        if *reuse_parent_tree_if_possible {
            if let Some(only_parent) = patch_commit.get_only_parent() {
                if only_parent.get_tree()?.get_oid() == target_commit.get_tree()?.get_oid() {
                    // If this patch is being applied to the same commit it was
                    // originally based on, then we can skip cherry-picking
                    // altogether, and use its tree directly. This is common e.g.
                    // when only rewording a commit message.
                    return Ok(Ok(patch_commit.get_tree()?));
                }
            };
        }

        let changed_pathbufs = self
            .get_paths_touched_by_commit(patch_commit)?
            .ok_or_else(|| {
                eyre::eyre!("Could not get paths touched by commit: {:?}", &patch_commit)
            })?
            .into_iter()
            .collect_vec();
        let changed_paths = changed_pathbufs.iter().map(PathBuf::borrow).collect_vec();

        let dehydrated_patch_commit =
            self.dehydrate_commit(patch_commit, changed_paths.as_slice(), true)?;
        let dehydrated_target_commit =
            self.dehydrate_commit(target_commit, changed_paths.as_slice(), false)?;

        let rebased_index =
            self.cherry_pick_commit(&dehydrated_patch_commit, &dehydrated_target_commit, 0)?;
        let rebased_tree = {
            if rebased_index.has_conflicts() {
                let conflicting_paths = {
                    let mut result = HashSet::new();
                    for conflict in rebased_index
                        .inner
                        .conflicts()
                        .wrap_err("Getting conflicting paths")?
                    {
                        let conflict = conflict.wrap_err("Getting conflicting path")?;
                        if let Some(ancestor) = conflict.ancestor {
                            result
                                .insert(PathBuf::from(OsStrBytes::from_raw_bytes(ancestor.path)?));
                        }
                        if let Some(our) = conflict.our {
                            result.insert(PathBuf::from(OsStrBytes::from_raw_bytes(our.path)?));
                        }
                        if let Some(their) = conflict.their {
                            result.insert(PathBuf::from(OsStrBytes::from_raw_bytes(their.path)?));
                        }
                    }
                    result
                };

                if conflicting_paths.is_empty() {
                    warn!("BUG: A merge conflict was detected, but there were no entries in `conflicting_paths`. Maybe the wrong index entry was used?")
                }

                return Ok(Err(CherryPickFastError::MergeConflict {
                    conflicting_paths,
                }));
            }
            let rebased_entries: HashMap<PathBuf, Option<(NonZeroOid, i32)>> = changed_pathbufs
                .into_iter()
                .map(|changed_path| {
                    let value = match rebased_index.get_entry(&changed_path) {
                        Some(IndexEntry {
                            oid: MaybeZeroOid::Zero,
                            file_mode: _,
                        }) => {
                            warn!(
                                ?patch_commit,
                                ?changed_path,
                                "BUG: index entry was zero. \
                                This probably indicates that a removed path \
                                was not handled correctly."
                            );
                            None
                        }
                        Some(IndexEntry {
                            oid: MaybeZeroOid::NonZero(oid),
                            file_mode,
                        }) => {
                            // `libgit2` uses u32 for file modes in index
                            // entries, but i32 for file modes in tree entries
                            // for some reason.
                            let file_mode: i32 = file_mode
                                .try_into()
                                .expect("Could not convert file mode from u32 to i32");
                            Some((oid, file_mode))
                        }
                        None => None,
                    };
                    (changed_path, value)
                })
                .collect();
            let rebased_tree_oid =
                hydrate_tree(self, Some(&target_commit.get_tree()?), rebased_entries)?;
            self.find_tree(rebased_tree_oid)?
                .ok_or_else(|| eyre::eyre!("Could not find just-hydrated tree"))?
        };
        Ok(Ok(rebased_tree))
    }

    #[instrument]
    fn dehydrate_commit(
        &self,
        commit: &Commit,
        changed_paths: &[&Path],
        base_on_parent: bool,
    ) -> eyre::Result<Commit> {
        let tree = commit.get_tree()?;
        let dehydrated_tree_oid = dehydrate_tree(self, &tree, changed_paths)?;
        let dehydrated_tree = self
            .find_tree(dehydrated_tree_oid)?
            .ok_or_else(|| eyre::eyre!("Could not find just-dehydrated tree"))?;

        let signature = Signature {
            inner: git2::Signature::new(
                "git-branchless",
                "git-branchless@example.com",
                &git2::Time::new(0, 0),
            )?,
        };
        let message = format!(
            "generated by git-branchless: temporary dehydrated commit \
                \
                This commit was originally: {:?}",
            commit.get_oid()
        );

        let parents = if base_on_parent {
            match commit.get_only_parent() {
                Some(parent) => {
                    let dehydrated_parent = self.dehydrate_commit(&parent, changed_paths, false)?;
                    vec![dehydrated_parent]
                }
                None => vec![],
            }
        } else {
            vec![]
        };
        let dehydrated_commit_oid = self
            .create_commit(
                None,
                &signature,
                &signature,
                &message,
                &dehydrated_tree,
                parents.iter().collect_vec(),
            )
            .wrap_err("Dehydrating commit")?;
        let dehydrated_commit = self.find_commit_or_fail(dehydrated_commit_oid)?;
        Ok(dehydrated_commit)
    }

    /// Look up the tree with the given OID. Returns `None` if not found.
    #[instrument]
    pub fn find_tree(&self, oid: NonZeroOid) -> eyre::Result<Option<Tree>> {
        match self.inner.find_tree(oid.inner) {
            Ok(tree) => Ok(Some(Tree { inner: tree })),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => Err(wrap_git_error(err)),
        }
    }

    /// Like `find_tree`, but raises a generic error if the commit could not
    /// be found.
    #[instrument]
    pub fn find_tree_or_fail(&self, oid: NonZeroOid) -> eyre::Result<Tree> {
        match self.find_tree(oid) {
            Ok(Some(tree)) => Ok(tree),
            Ok(None) => eyre::bail!("Could not find tree with OID: {:?}", oid),
            Err(err) => Err(err),
        }
    }

    /// Write the provided in-memory index as a tree into Git`s object database.
    /// There must be no merge conflicts in the index.
    #[instrument]
    pub fn write_index_to_tree(&self, index: &mut Index) -> eyre::Result<NonZeroOid> {
        let oid = index
            .inner
            .write_tree_to(&self.inner)
            .map_err(wrap_git_error)?;
        Ok(make_non_zero_oid(oid))
    }

    /// Amends the provided parent commit in memory and returns the resulting tree.
    ///
    /// Only amends the files provided in the options, and only supports amending from
    /// either the working tree or the index, but not both.
    ///
    /// See `Repo::cherry_pick_fast` for motivation for performing the operation
    /// in-memory.
    #[instrument]
    pub fn amend_fast(
        &self,
        parent_commit: &Commit,
        opts: &AmendFastOptions,
    ) -> eyre::Result<Tree> {
        let parent_commit_pathbufs = self
            .get_paths_touched_by_commit(parent_commit)?
            .ok_or_else(|| {
                eyre::eyre!(
                    "Could not get paths touched by commit: {:?}",
                    &parent_commit
                )
            })?
            .into_iter()
            .collect_vec();
        let changed_paths: Vec<PathBuf> = {
            let mut result: HashSet<PathBuf> = parent_commit_pathbufs.into_iter().collect();
            match opts {
                AmendFastOptions::FromIndex { paths } => result.extend(paths.iter().cloned()),
                AmendFastOptions::FromWorkingCopy { ref status_entries } => {
                    for entry in status_entries {
                        result.extend(entry.paths().iter().cloned());
                    }
                }
            };
            result.into_iter().collect_vec()
        };
        let changed_paths = changed_paths
            .iter()
            .map(|path| path.as_path())
            .collect_vec();

        let dehydrated_parent =
            self.dehydrate_commit(parent_commit, changed_paths.as_slice(), true)?;
        let dehydrated_parent_tree = dehydrated_parent.get_tree()?;

        let repo_path = self
            .get_working_copy_path()
            .ok_or_else(|| eyre::eyre!("unable to get repo working copy path"))?;
        let new_tree_entries: HashMap<PathBuf, Option<(NonZeroOid, i32)>> = match opts {
            AmendFastOptions::FromWorkingCopy { status_entries } => status_entries
                .iter()
                .map(|entry| {
                    entry.paths().into_iter().map(move |path| {
                        let file_path = &repo_path.join(&path);
                        // Try to create a new blob OID based on the current on-disk
                        // contents of the file in the working copy.
                        match self.inner.blob_path(file_path) {
                            Ok(oid) => Ok((
                                path,
                                Some((make_non_zero_oid(oid), entry.working_copy_file_mode.into())),
                            )),
                            // If the file doesn't exist, it needs to be explicitly marked as
                            // such, as a tombstone to override if the file exists in the parent tree.
                            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok((path, None)),
                            Err(other) => Err(eyre::eyre!(other)),
                        }
                    })
                })
                .flatten()
                .collect::<Result<HashMap<_, _>, _>>()?,
            AmendFastOptions::FromIndex { paths } => {
                let index = self.get_index()?;
                paths
                    .iter()
                    .filter_map(|path| match index.get_entry(path) {
                        Some(IndexEntry {
                            oid: MaybeZeroOid::Zero,
                            ..
                        }) => {
                            warn!(?path, "index entry was zero");
                            None
                        }
                        Some(IndexEntry {
                            oid: MaybeZeroOid::NonZero(oid),
                            file_mode,
                            ..
                        }) => Some((
                            path.clone(),
                            Some((
                                oid,
                                file_mode
                                    .try_into()
                                    .expect("Could not convert file mode from u32 to i32"),
                            )),
                        )),
                        None => Some((path.clone(), None)),
                    })
                    .collect::<HashMap<_, _>>()
            }
        };

        // Merge the new path entries into the existing set of parent tree.
        let amended_tree_entries: HashMap<PathBuf, Option<(NonZeroOid, i32)>> = changed_paths
            .into_iter()
            .map(|changed_path| {
                let value = match new_tree_entries.get(changed_path) {
                    Some(new_tree_entry) => new_tree_entry.as_ref().copied(),
                    None => match dehydrated_parent_tree.get_path(changed_path) {
                        Ok(Some(entry)) => Some((entry.get_oid(), entry.get_filemode())),
                        Ok(None) => None,
                        Err(err) => eyre::bail!(
                            "getting path {:?} from dehydrated parent index: {}",
                            changed_path,
                            err
                        ),
                    },
                };
                Ok((changed_path.into(), value))
            })
            .collect::<eyre::Result<_>>()?;

        let amended_tree_oid =
            hydrate_tree(self, Some(&parent_commit.get_tree()?), amended_tree_entries)?;
        let amended_tree = self.find_tree_or_fail(amended_tree_oid)?;

        Ok(amended_tree)
    }
}

/// The signature of a commit, identifying who it was made by and when it was made.
pub struct Signature<'repo> {
    inner: git2::Signature<'repo>,
}

impl std::fmt::Debug for Signature<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<Signature>")
    }
}

impl<'repo> Signature<'repo> {
    /// Update the timestamp of this signature to a new time.
    #[instrument]
    pub fn update_timestamp(self, now: SystemTime) -> eyre::Result<Signature<'repo>> {
        let seconds: i64 = now
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs()
            .try_into()?;
        let time = git2::Time::new(seconds, self.inner.when().offset_minutes());
        let name = match self.inner.name() {
            Some(name) => name,
            None => eyre::bail!(
                "Could not decode signature name: {:?}",
                self.inner.name_bytes()
            ),
        };
        let email = match self.inner.email() {
            Some(email) => email,
            None => eyre::bail!(
                "Could not decode signature email: {:?}",
                self.inner.email_bytes()
            ),
        };
        let signature = git2::Signature::new(name, email, &time)?;
        Ok(Signature { inner: signature })
    }

    /// Get the time when this signature was applied.
    pub fn get_time(&self) -> git2::Time {
        self.inner.when()
    }

    /// Return the friendly formatted name and email of the signature.
    pub fn friendly_describe(&self) -> Option<String> {
        let name = self.inner.name();
        let email = self.inner.email().map(|email| format!("<{}>", email));
        match (name, email) {
            (Some(name), Some(email)) => Some(format!("{} {}", name, email)),
            (Some(name), _) => Some(name.into()),
            (_, Some(email)) => Some(email),
            _ => None,
        }
    }
}

pub struct IndexEntry {
    oid: MaybeZeroOid,
    file_mode: u32,
}

pub struct Index {
    inner: git2::Index,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<Index>")
    }
}

impl Index {
    pub fn has_conflicts(&self) -> bool {
        self.inner.has_conflicts()
    }

    pub fn get_entry(&self, path: &Path) -> Option<IndexEntry> {
        self.inner.get_path(path, 0).map(|entry| IndexEntry {
            oid: entry.id.into(),
            file_mode: entry.mode,
        })
    }
}

/// A diff between two trees/commits.
pub struct Diff<'repo> {
    inner: git2::Diff<'repo>,
}

/// A checksum of the diff induced by a given commit, used for duplicate commit
/// detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PatchId {
    patch_id: git2::Oid,
}

/// Represents a commit object in the Git object database.
#[derive(Clone, Debug)]
pub struct Commit<'repo> {
    inner: git2::Commit<'repo>,
}

impl<'repo> Commit<'repo> {
    /// Get the object ID of the commit.
    pub fn get_oid(&self) -> NonZeroOid {
        NonZeroOid {
            inner: self.inner.id(),
        }
    }

    /// Get the object IDs of the parents of this commit.
    pub fn get_parent_oids(&self) -> Vec<NonZeroOid> {
        self.inner.parent_ids().map(make_non_zero_oid).collect()
    }

    /// Get the parent OID of this commit if there is exactly one parent, or
    /// `None` otherwise.
    pub fn get_only_parent_oid(&self) -> Option<NonZeroOid> {
        match self.get_parent_oids().as_slice() {
            [] | [_, _, ..] => None,
            [only_parent_oid] => Some(*only_parent_oid),
        }
    }

    /// Get the number of parents of this commit.
    pub fn get_parent_count(&self) -> usize {
        self.inner.parent_count()
    }

    /// Get the parent commits of this commit.
    pub fn get_parents(&self) -> Vec<Commit<'repo>> {
        self.inner
            .parents()
            .map(|commit| Commit { inner: commit })
            .collect()
    }

    /// Get the parent of this commit if there is exactly one parent, or `None`
    /// otherwise.
    pub fn get_only_parent(&self) -> Option<Commit<'repo>> {
        match self.get_parents().as_slice() {
            [] | [_, _, ..] => None,
            [only_parent] => Some(only_parent.clone()),
        }
    }

    /// Get the commit time of this commit.
    pub fn get_time(&self) -> git2::Time {
        self.inner.time()
    }

    /// Get the summary (first line) of the commit message.
    pub fn get_summary(&self) -> eyre::Result<OsString> {
        match self.inner.summary_bytes() {
            Some(summary) => Ok(OsString::from_raw_vec(summary.into())?),
            None => eyre::bail!("Could not read summary for commit: {:?}", self.get_oid()),
        }
    }

    /// Get the commit message with some whitespace trimmed.
    pub fn get_message_pretty(&self) -> eyre::Result<OsString> {
        let message = OsString::from_raw_vec(self.inner.message_bytes().into())?;
        Ok(message)
    }

    /// Get the commit message, without any whitespace trimmed.
    pub fn get_message_raw(&self) -> eyre::Result<OsString> {
        let message = OsString::from_raw_vec(self.inner.message_raw_bytes().into())?;
        Ok(message)
    }

    /// Get the author of this commit.
    pub fn get_author(&self) -> Signature {
        Signature {
            inner: self.inner.author(),
        }
    }

    /// Get the committer of this commit.
    pub fn get_committer(&self) -> Signature {
        Signature {
            inner: self.inner.committer(),
        }
    }

    /// Get the `Tree` object associated with this commit.
    #[instrument]
    pub fn get_tree(&self) -> eyre::Result<Tree> {
        let tree = self.inner.tree().wrap_err("Getting tree object")?;
        Ok(Tree { inner: tree })
    }

    /// Print a one-line description of this commit containing its OID and
    /// summary.
    #[instrument]
    pub fn friendly_describe(&self) -> eyre::Result<StyledString> {
        let description = render_node_descriptors(
            &NodeObject::Commit {
                commit: self.clone(),
            },
            &mut [
                &mut CommitOidDescriptor::new(true)?,
                &mut CommitMessageDescriptor::new()?,
            ],
        )?;
        Ok(description)
    }

    /// Get a multi-line description of this commit containing information about
    /// its OID, author, commit time, and message.
    #[instrument]
    pub fn friendly_preview(&self) -> eyre::Result<StyledString> {
        let commit_time = Utc.timestamp(self.get_time().seconds(), 0);
        let commit_time: DateTime<Local> = DateTime::from(commit_time);
        let preview = StyledStringBuilder::from_lines(vec![
            StyledStringBuilder::new()
                .append_styled(
                    format!("Commit:\t{}", self.get_oid()),
                    BaseColor::Yellow.light(),
                )
                .build(),
            StyledString::styled(
                format!(
                    "Author:\t{}",
                    self.get_author()
                        .friendly_describe()
                        .unwrap_or_else(|| "".into())
                ),
                BaseColor::Magenta.light(),
            ),
            StyledString::styled(format!("Date:\t{}", commit_time), BaseColor::Green.light()),
            StyledString::plain(textwrap::indent(
                &self.get_message_pretty()?.to_string_lossy(),
                "    ",
            )),
        ]);
        Ok(preview)
    }

    /// Determine if the current commit is empty (has no changes compared to its
    /// parent).
    pub fn is_empty(&self) -> bool {
        match self.get_parents().as_slice() {
            [] => false,
            [parent_commit] => self.inner.tree_id() == parent_commit.inner.tree_id(),
            _ => false,
        }
    }

    /// Determine if this commit added, removed, or changed the entry at the
    /// provided file path.
    pub fn contains_touched_path(&self, path: &Path) -> eyre::Result<Option<bool>> {
        let parent = match self.get_only_parent() {
            None => return Ok(None),
            Some(parent) => parent,
        };
        let parent_tree = parent.get_tree()?;
        let current_tree = self.get_tree()?;
        let parent_oid = parent_tree.get_oid_for_path(path)?;
        let current_oid = current_tree.get_oid_for_path(path)?;
        match (parent_oid, current_oid) {
            (None, None) => Ok(Some(false)),
            (None, Some(_)) | (Some(_), None) => Ok(Some(true)),
            (Some(parent_oid), Some(current_oid)) => Ok(Some(parent_oid != current_oid)),
        }
    }

    /// Amend this existing commit.
    /// Returns the OID of the resulting new commit.
    #[instrument]
    pub fn amend_commit(
        &self,
        update_ref: Option<&str>,
        author: Option<&Signature>,
        committer: Option<&Signature>,
        message: Option<&str>,
        tree: Option<&Tree>,
    ) -> eyre::Result<NonZeroOid> {
        let oid = self
            .inner
            .amend(
                update_ref,
                author.map(|author| &author.inner),
                committer.map(|committer| &committer.inner),
                None,
                message,
                tree.map(|tree| &tree.inner),
            )
            .map_err(wrap_git_error)?;
        Ok(make_non_zero_oid(oid))
    }
}

/// The target of a reference.
#[derive(Debug, PartialEq, Eq)]
pub enum ReferenceTarget<'a> {
    /// The reference points directly to an object. This is the case for most
    /// references, such as branches.
    Direct {
        /// The OID of the pointed-to object.
        oid: MaybeZeroOid,
    },

    /// The reference points to another reference with the given name.
    Symbolic {
        /// The name of the pointed-to reference.
        reference_name: Cow<'a, OsStr>,
    },
}

/// Represents a reference to an object.
pub struct Reference<'repo> {
    inner: git2::Reference<'repo>,
}

impl std::fmt::Debug for Reference<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.inner.name() {
            Some(name) => write!(f, "<Reference name={:?}>", name),
            None => write!(f, "<Reference name={:?}>", self.inner.name_bytes()),
        }
    }
}

impl<'repo> Reference<'repo> {
    /// Determine if the given name is a valid name for a reference.
    pub fn is_valid_name(name: &str) -> bool {
        git2::Reference::is_valid_name(name)
    }

    /// Given a reference name which is an OID, convert the string into an `Oid`
    /// object. If the `Oid` was zero, returns `None`.
    #[instrument]
    pub fn name_to_oid(ref_name: &OsStr) -> eyre::Result<Option<NonZeroOid>> {
        let oid: MaybeZeroOid = ref_name.try_into()?;
        match oid {
            MaybeZeroOid::NonZero(oid) => Ok(Some(oid)),
            MaybeZeroOid::Zero => Ok(None),
        }
    }

    /// Get the name of this reference.
    #[instrument]
    pub fn get_name(&self) -> eyre::Result<OsString> {
        let name = OsStringBytes::from_raw_vec(self.inner.name_bytes().into())
            .wrap_err("Decoding reference name")?;
        Ok(name)
    }
    /// Get the commit object pointed to by this reference. Returns `None` if
    /// the object pointed to by the reference is a different kind of object.
    #[instrument]
    pub fn peel_to_commit(&self) -> eyre::Result<Option<Commit<'repo>>> {
        let object = match self.inner.peel(git2::ObjectType::Commit) {
            Ok(object) => object,
            Err(err) if err.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        match object.into_commit() {
            Ok(commit) => Ok(Some(Commit { inner: commit })),
            Err(_) => Ok(None),
        }
    }

    /// Delete the reference.
    #[instrument]
    pub fn delete(&mut self) -> eyre::Result<()> {
        self.inner.delete().wrap_err("Deleting reference")?;
        Ok(())
    }
}

/// Determine what kind of branch a reference is, given its name. The returned
/// `suffix` value is converted to a `String` to be rendered to the screen, so
/// it may have lost some information if the reference name had unusual
/// characters.
#[derive(Debug)]
pub enum CategorizedReferenceName<'a> {
    /// The reference represents a local branch.
    LocalBranch {
        /// The full name of the reference.
        name: &'a OsStr,

        /// The string `refs/heads/`.
        prefix: &'static str,
    },

    /// The reference represents a remote branch.
    RemoteBranch {
        /// The full name of the reference.
        name: &'a OsStr,

        /// The string `refs/remotes/`.
        prefix: &'static str,
    },

    /// Some other kind of reference which isn't a branch at all.
    OtherRef {
        /// The full name of the reference.
        name: &'a OsStr,
    },
}

impl<'a> CategorizedReferenceName<'a> {
    /// Categorize the provided reference name.
    pub fn new(name: &'a OsStr) -> Self {
        let bytes = name.to_raw_bytes();
        if bytes.starts_with(b"refs/heads/") {
            Self::LocalBranch {
                name,
                prefix: "refs/heads/",
            }
        } else if bytes.starts_with(b"refs/remotes/") {
            Self::RemoteBranch {
                name,
                prefix: "refs/remotes/",
            }
        } else {
            Self::OtherRef { name }
        }
    }

    /// Remove the prefix from the reference name. May raise an error if the
    /// result couldn't be encoded as an `OsString` (probably shouldn't
    /// happen?).
    #[instrument]
    pub fn remove_prefix(&self) -> eyre::Result<OsString> {
        let (name, prefix): (_, &'static str) = match self {
            Self::LocalBranch { name, prefix } => (name, prefix),
            Self::RemoteBranch { name, prefix } => (name, prefix),
            Self::OtherRef { name } => (name, ""),
        };
        let bytes = name.to_raw_bytes();
        let bytes = match bytes.strip_prefix(prefix.as_bytes()) {
            Some(bytes) => Vec::from(bytes),
            None => Vec::from(bytes),
        };
        let result = OsString::from_raw_vec(bytes)?;
        Ok(result)
    }

    /// Render the full name of the reference, including its prefix, lossily as
    /// a `String`.
    pub fn render_full(&self) -> String {
        let name = match self {
            Self::LocalBranch { name, prefix: _ } => name,
            Self::RemoteBranch { name, prefix: _ } => name,
            Self::OtherRef { name } => name,
        };
        name.to_string_lossy().into_owned()
    }

    /// Render only the suffix of the reference name lossily as a `String`. The
    /// caller will usually check the type of reference and add additional
    /// information to the reference name.
    pub fn render_suffix(&self) -> String {
        let (name, prefix): (_, &'static str) = match self {
            Self::LocalBranch { name, prefix } => (name, prefix),
            Self::RemoteBranch { name, prefix } => (name, prefix),
            Self::OtherRef { name } => (name, ""),
        };
        let name = name.to_string_lossy();
        match name.strip_prefix(prefix) {
            Some(name) => name.to_string(),
            None => name.into_owned(),
        }
    }

    /// Render the reference name lossily, and prepend a helpful string like
    /// `branch` to the description.
    pub fn friendly_describe(&self) -> String {
        let name = self.render_suffix();
        let name = match self {
            CategorizedReferenceName::LocalBranch { .. } => {
                format!("branch {}", name)
            }
            CategorizedReferenceName::RemoteBranch { .. } => {
                format!("remote branch {}", name)
            }
            CategorizedReferenceName::OtherRef { .. } => format!("ref {}", name),
        };
        name
    }
}

type BranchType = git2::BranchType;

/// Represents a Git branch.
pub struct Branch<'repo> {
    inner: git2::Branch<'repo>,
}

impl<'repo> Branch<'repo> {
    /// Get the OID pointed to by the branch. Returns `None` if the branch is
    /// not a direct reference (which is unusual).
    pub fn get_oid(&self) -> eyre::Result<Option<NonZeroOid>> {
        Ok(self.inner.get().target().map(make_non_zero_oid))
    }

    /// Convert the branch into its underlying `Reference`.
    pub fn into_reference(self) -> Reference<'repo> {
        Reference {
            inner: self.inner.into_reference(),
        }
    }
}

/// A Git file status indicator.
/// See <https://git-scm.com/docs/git-status#_short_format>.
#[allow(missing_docs)]
#[derive(Debug, PartialEq, Eq)]
pub enum FileStatus {
    Unmodified,
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
    Ignored,
}

impl From<u8> for FileStatus {
    fn from(status: u8) -> Self {
        match status {
            b'.' => FileStatus::Unmodified,
            b'M' => FileStatus::Modified,
            b'A' => FileStatus::Added,
            b'D' => FileStatus::Deleted,
            b'R' => FileStatus::Renamed,
            b'C' => FileStatus::Copied,
            b'U' => FileStatus::Unmerged,
            b'?' => FileStatus::Untracked,
            b'!' => FileStatus::Ignored,
            _ => {
                warn!(?status, "invalid status indicator");
                FileStatus::Untracked
            }
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FileMode {
    Unreadable,
    Tree,
    Blob,
    BlobExecutable,
    Link,
    Commit,
}

impl From<git2::FileMode> for FileMode {
    fn from(file_mode: git2::FileMode) -> Self {
        match file_mode {
            git2::FileMode::Blob => FileMode::Blob,
            git2::FileMode::BlobExecutable => FileMode::BlobExecutable,
            git2::FileMode::Commit => FileMode::Commit,
            git2::FileMode::Link => FileMode::Link,
            git2::FileMode::Tree => FileMode::Tree,
            git2::FileMode::Unreadable => FileMode::Unreadable,
        }
    }
}

impl From<FileMode> for i32 {
    fn from(file_mode: FileMode) -> Self {
        match file_mode {
            FileMode::Blob => git2::FileMode::Blob.into(),
            FileMode::BlobExecutable => git2::FileMode::BlobExecutable.into(),
            FileMode::Commit => git2::FileMode::Commit.into(),
            FileMode::Link => git2::FileMode::Link.into(),
            FileMode::Tree => git2::FileMode::Tree.into(),
            FileMode::Unreadable => git2::FileMode::Unreadable.into(),
        }
    }
}

impl FromStr for FileMode {
    type Err = eyre::Error;

    // Parses the string representation of a filemode for a status entry.
    // Git only supports a small subset of Unix octal file mode permissions.
    // See http://git-scm.com/book/en/v2/Git-Internals-Git-Objects
    fn from_str(file_mode: &str) -> eyre::Result<Self> {
        let file_mode = match file_mode {
            "000000" => FileMode::Unreadable,
            "040000" => FileMode::Tree,
            "100644" => FileMode::Blob,
            "100755" => FileMode::BlobExecutable,
            "120000" => FileMode::Link,
            "160000" => FileMode::Commit,
            _ => eyre::bail!("unknown file mode: {}", file_mode),
        };
        Ok(file_mode)
    }
}

/// The status of a file in the repo.
#[derive(Debug, PartialEq, Eq)]
pub struct StatusEntry {
    /// The status of the file in the index.
    pub index_status: FileStatus,
    /// The status of the file in the working copy.
    pub working_copy_status: FileStatus,
    /// The file mode of the file in the working copy.
    pub working_copy_file_mode: FileMode,
    /// The file path.
    pub path: PathBuf,
    /// The original path of the file (for renamed files).
    pub orig_path: Option<PathBuf>,
}

impl StatusEntry {
    /// Returns the paths associated with the status entry.
    pub fn paths(&self) -> Vec<PathBuf> {
        let mut result = vec![self.path.clone()];
        if let Some(orig_path) = &self.orig_path {
            result.push(orig_path.clone());
        }
        result
    }
}

impl TryFrom<&[u8]> for StatusEntry {
    type Error = eyre::Error;

    #[instrument]
    fn try_from(line: &[u8]) -> eyre::Result<StatusEntry> {
        lazy_static! {
            /// Parses an entry of the git porcelain v2 status format.
            /// See https://git-scm.com/docs/git-status#_porcelain_format_version_2
            static ref STATUS_PORCELAIN_V2_REGEXP: Regex = Regex::new(concat!(
                r#"^(1|2) (?P<index_status>[\w.])(?P<working_copy_status>[\w.]) "#, // Prefix and status indicators.
                r#"[\w.]+ "#,                                                       // Submodule state.
                r#"(\d{6} ){2}(?P<working_copy_filemode>\d{6}) "#,                  // HEAD, Index, and Working Copy file modes.
                r#"([\w\d]+ ){2,3}"#,                                               // HEAD and Index object IDs, and optionally the rename/copy score.
                r#"(?P<path>[^\x00]+)(\x00(?P<orig_path>[^\x00]+))?$"#              // Path and original path (for renames/copies).
            ))
            .expect("porcelain v2 status line regex");
        }

        let status_line_parts = STATUS_PORCELAIN_V2_REGEXP
            .captures(line)
            .ok_or_else(|| eyre::eyre!("unable to parse status line into parts"))?;

        let index_status: FileStatus = status_line_parts
            .name("index_status")
            .and_then(|m| m.as_bytes().iter().next().copied())
            .ok_or_else(|| eyre::eyre!("no index status indicator"))?
            .into();
        let working_copy_status: FileStatus = status_line_parts
            .name("working_copy_status")
            .and_then(|m| m.as_bytes().iter().next().copied())
            .ok_or_else(|| eyre::eyre!("no working copy status indicator"))?
            .into();
        let working_copy_file_mode = status_line_parts
            .name("working_copy_filemode")
            .ok_or_else(|| eyre::eyre!("no working copy filemode in status line"))
            .and_then(|m| {
                std::str::from_utf8(m.as_bytes())
                    .map_err(|err| {
                        eyre::eyre!("unable to decode working copy file mode: {:?}", err)
                    })
                    .and_then(|working_copy_file_mode| working_copy_file_mode.parse::<FileMode>())
            })?;
        let path = status_line_parts
            .name("path")
            .ok_or_else(|| eyre::eyre!("no path in status line"))?
            .as_bytes();
        let orig_path = status_line_parts
            .name("orig_path")
            .map(|orig_path| orig_path.as_bytes());

        Ok(StatusEntry {
            index_status,
            working_copy_status,
            working_copy_file_mode,
            path: PathBuf::from(OsStrBytes::from_raw_bytes(path)?),
            orig_path: orig_path.map(|orig_path| {
                OsStrBytes::from_raw_bytes(orig_path)
                    .map(PathBuf::from)
                    .expect("unable to convert orig_path to PathBuf")
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::make_git;

    use super::*;

    #[test]
    fn test_parse_git_version_output() {
        assert_eq!(
            "git version 12.34.56".parse::<GitVersion>().unwrap(),
            GitVersion(12, 34, 56)
        );
        assert_eq!(
            "git version 12.34.56\n".parse::<GitVersion>().unwrap(),
            GitVersion(12, 34, 56)
        );
        assert_eq!(
            "git version 12.34.56.78.abcdef"
                .parse::<GitVersion>()
                .unwrap(),
            GitVersion(12, 34, 56)
        );

        // See https://github.com/arxanas/git-branchless/issues/69
        assert_eq!(
            "git version 2.33.0-rc0".parse::<GitVersion>().unwrap(),
            GitVersion(2, 33, 0)
        );

        // See https://github.com/arxanas/git-branchless/issues/85
        assert_eq!(
            "git version 2.33.GIT".parse::<GitVersion>().unwrap(),
            GitVersion(2, 33, 0)
        );
    }

    #[test]
    fn test_cherry_pick_fast() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        git.run(&["checkout", "-b", "foo"])?;
        let test1_oid = git.commit_file_with_contents("test1", 1, "test1 contents")?;
        git.run(&["checkout", "master"])?;
        let initial2_oid =
            git.commit_file_with_contents("initial", 2, "updated initial contents")?;

        let repo = git.get_repo()?;
        let test1_commit = repo.find_commit_or_fail(test1_oid)?;
        let initial2_commit = repo.find_commit_or_fail(initial2_oid)?;
        let tree = repo.cherry_pick_fast(
            &test1_commit,
            &initial2_commit,
            &CherryPickFastOptions {
                reuse_parent_tree_if_possible: false,
            },
        )?;

        insta::assert_debug_snapshot!(tree, @r###"
        Ok(
            Tree {
                inner: Tree {
                    id: 367f91ddd5df2d1c18742ce3f09b4944944cac3a,
                },
            },
        )
        "###);

        let tree = tree.unwrap();
        insta::assert_debug_snapshot!(tree.inner.iter().map(|entry| entry.name().unwrap().to_string()).collect_vec(), @r###"
        [
            "initial.txt",
            "test1.txt",
        ]
        "###);

        Ok(())
    }

    #[test]
    fn test_amend_fast_from_index() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        git.run(&["checkout", "master"])?;
        let initial_oid = git.commit_file_with_contents("initial", 2, "initial contents")?;
        git.write_file("initial", "updated contents")?;

        let repo = git.get_repo()?;
        let initial_commit = repo.find_commit_or_fail(initial_oid)?;

        let tree = initial_commit.get_tree()?;
        insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 01deb7745d411223bbf6b9cb1abaeed451bb25a0,
            },
        }
        "###);
        insta::assert_debug_snapshot!(tree.inner.iter().map(|entry| (entry.name().unwrap().to_string(), entry.id().to_string())).collect_vec(), @r###"
        [
            (
                "initial.txt",
                "5c41c3d7e736911dbbd53d62c10292b9bc78f838",
            ),
        ]
        "###);

        let tree = repo.amend_fast(
            &initial_commit,
            &AmendFastOptions::FromIndex {
                paths: vec!["initial.txt".into()],
            },
        )?;

        insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 01deb7745d411223bbf6b9cb1abaeed451bb25a0,
            },
        }
        "###);
        insta::assert_debug_snapshot!(tree.inner.iter().map(|entry| (entry.name().unwrap().to_string(), entry.id().to_string())).collect_vec(), @r###"
        [
            (
                "initial.txt",
                "5c41c3d7e736911dbbd53d62c10292b9bc78f838",
            ),
        ]
        "###);

        git.run(&["add", "initial.txt"])?;
        let tree = repo.amend_fast(
            &initial_commit,
            &AmendFastOptions::FromIndex {
                paths: vec!["initial.txt".into()],
            },
        )?;

        insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 01deb7745d411223bbf6b9cb1abaeed451bb25a0,
            },
        }
        "###);
        insta::assert_debug_snapshot!(tree.inner.iter().map(|entry| (entry.name().unwrap().to_string(), entry.id().to_string())).collect_vec(), @r###"
        [
            (
                "initial.txt",
                "5c41c3d7e736911dbbd53d62c10292b9bc78f838",
            ),
        ]
        "###);

        Ok(())
    }

    #[test]
    fn test_amend_fast_from_working_tree() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        git.run(&["checkout", "master"])?;
        let initial_oid = git.commit_file_with_contents("initial", 2, "initial contents")?;
        git.write_file("initial", "updated contents")?;

        let repo = git.get_repo()?;
        let initial_commit = repo.find_commit_or_fail(initial_oid)?;
        let tree = repo.amend_fast(
            &initial_commit,
            &AmendFastOptions::FromWorkingCopy {
                status_entries: vec![StatusEntry {
                    index_status: FileStatus::Renamed,
                    working_copy_status: FileStatus::Unmodified,
                    working_copy_file_mode: FileMode::Blob,
                    path: "initial.txt".into(),
                    orig_path: None,
                }],
            },
        )?;

        insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 1c15b79a72c3285df172fcfdaceedb7259283eb5,
            },
        }
        "###);
        insta::assert_debug_snapshot!(tree.inner.iter().map(|entry| (entry.name().unwrap().to_string(), entry.id().to_string())).collect_vec(), @r###"
        [
            (
                "initial.txt",
                "53cd9398c8a2d92f18d279c6cad3f5dde67235e7",
            ),
        ]
        "###);

        git.write_file("file2", "another file")?;
        git.write_file("initial", "updated contents again")?;
        let tree = repo.amend_fast(
            &initial_commit,
            &AmendFastOptions::FromWorkingCopy {
                status_entries: vec![StatusEntry {
                    index_status: FileStatus::Unmodified,
                    working_copy_status: FileStatus::Added,
                    working_copy_file_mode: FileMode::Blob,
                    path: "file2.txt".into(),
                    orig_path: None,
                }],
            },
        )?;
        insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 1a9fbbecd825881c3e79f0fb194a1c1e1104fe0f,
            },
        }
        "###);
        insta::assert_debug_snapshot!(tree.inner.iter().map(|entry| (entry.name().unwrap().to_string(), entry.id().to_string())).collect_vec(), @r###"
        [
            (
                "file2.txt",
                "cdcb28483da7783a8b505a074c50632a5481a69b",
            ),
            (
                "initial.txt",
                "5c41c3d7e736911dbbd53d62c10292b9bc78f838",
            ),
        ]
        "###);

        git.delete_file("initial")?;
        let tree = repo.amend_fast(
            &initial_commit,
            &AmendFastOptions::FromWorkingCopy {
                status_entries: vec![StatusEntry {
                    index_status: FileStatus::Unmodified,
                    working_copy_status: FileStatus::Deleted,
                    working_copy_file_mode: FileMode::Blob,
                    path: "initial.txt".into(),
                    orig_path: None,
                }],
            },
        )?;
        insta::assert_debug_snapshot!(tree, @r###"
        Tree {
            inner: Tree {
                id: 4b825dc642cb6eb9a060e54bf8d69288fbee4904,
            },
        }
        "###);
        insta::assert_debug_snapshot!(tree.inner.iter().map(|entry| (entry.name().unwrap().to_string(), entry.id().to_string())).collect_vec(), @"[]");

        Ok(())
    }

    #[test]
    fn test_parse_status_line() {
        assert_eq!(
            TryInto::<StatusEntry>::try_into(
                "1 .M N... 100644 100644 100644 51fcbe2362663a19d132767b69c2c7829023f3da 51fcbe2362663a19d132767b69c2c7829023f3da repo.rs".as_bytes(),
            ).unwrap(),
            StatusEntry {
                index_status: FileStatus::Unmodified,
                working_copy_status: FileStatus::Modified,
                path: "repo.rs".into(),
                orig_path: None,
                working_copy_file_mode: FileMode::Blob,
            }
        );

        assert_eq!(
            TryInto::<StatusEntry>::try_into(
                "1 A. N... 100755 100755 100755 51fcbe2362663a19d132767b69c2c7829023f3da 51fcbe2362663a19d132767b69c2c7829023f3da repo.rs".as_bytes(),
            ).unwrap(),
            StatusEntry {
                index_status: FileStatus::Added,
                working_copy_status: FileStatus::Unmodified,
                path: "repo.rs".into(),
                orig_path: None,
                working_copy_file_mode: FileMode::BlobExecutable,
            }
        );

        let entry: StatusEntry = TryInto::<StatusEntry>::try_into(
            "2 RD N... 100644 100644 100644 9daeafb9864cf43055ae93beb0afd6c7d144bfa4 9daeafb9864cf43055ae93beb0afd6c7d144bfa4 R100 new_file.rs\x00old_file.rs".as_bytes(),
        ).unwrap();
        assert_eq!(
            entry,
            StatusEntry {
                index_status: FileStatus::Renamed,
                working_copy_status: FileStatus::Deleted,
                path: "new_file.rs".into(),
                orig_path: Some("old_file.rs".into()),
                working_copy_file_mode: FileMode::Blob,
            }
        );
        assert_eq!(
            entry.paths(),
            vec![PathBuf::from("new_file.rs"), PathBuf::from("old_file.rs")]
        );
    }

    #[test]
    fn test_get_status() -> eyre::Result<()> {
        let git = make_git()?;
        let git_run_info = GitRunInfo {
            path_to_git: git.path_to_git.clone(),
            working_directory: git.repo_path.clone(),
            env: git.get_base_env(0).into_iter().collect(),
        };
        git.init_repo()?;
        git.commit_file("test1", 1)?;

        let repo = git.get_repo()?;

        let status = repo.get_status(&git_run_info, None)?;
        assert_eq!(status, vec![]);

        git.write_file("new_file", "another file")?;
        git.run(&["add", "new_file.txt"])?;
        git.write_file("untracked", "should not show up in status")?;
        git.delete_file("initial")?;
        git.run(&["mv", "test1.txt", "renamed.txt"])?;

        let status = repo.get_status(&git_run_info, None)?;
        assert_eq!(
            status,
            vec![
                StatusEntry {
                    index_status: FileStatus::Unmodified,
                    working_copy_status: FileStatus::Deleted,
                    working_copy_file_mode: FileMode::Unreadable,
                    path: "initial.txt".into(),
                    orig_path: None
                },
                StatusEntry {
                    index_status: FileStatus::Added,
                    working_copy_status: FileStatus::Unmodified,
                    working_copy_file_mode: FileMode::Blob,
                    path: "new_file.txt".into(),
                    orig_path: None
                },
                StatusEntry {
                    index_status: FileStatus::Renamed,
                    working_copy_status: FileStatus::Unmodified,
                    working_copy_file_mode: FileMode::Blob,
                    path: "renamed.txt".into(),
                    orig_path: Some("test1.txt".into())
                }
            ]
        );

        Ok(())
    }
}
