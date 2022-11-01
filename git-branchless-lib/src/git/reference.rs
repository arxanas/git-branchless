use std::borrow::Cow;
use std::ffi::OsStr;
use std::string::FromUtf8Error;

use thiserror::Error;
use tracing::instrument;

use crate::git::config::ConfigRead;
use crate::git::oid::make_non_zero_oid;
use crate::git::repo::{Error, Result};
use crate::git::{Commit, MaybeZeroOid, NonZeroOid, Repo};

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

#[derive(Debug, Error)]
pub enum ReferenceNameError {
    #[error("reference name was not valid UTF-8: {0}")]
    InvalidUtf8(FromUtf8Error),
}

/// The name of a reference, like `refs/heads/master`.
#[derive(Clone, Debug, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct ReferenceName(String);

impl ReferenceName {
    /// Create a reference name from the provided bytestring. Non-UTF-8 references are not supported.
    pub fn from_bytes(bytes: Vec<u8>) -> std::result::Result<ReferenceName, ReferenceNameError> {
        let reference_name = String::from_utf8(bytes).map_err(ReferenceNameError::InvalidUtf8)?;
        Ok(Self(reference_name))
    }

    /// View this reference name as a string. (This is a zero-cost conversion.)
    pub fn as_str(&self) -> &str {
        let Self(reference_name) = self;
        reference_name
    }
}

impl From<&str> for ReferenceName {
    fn from(s: &str) -> Self {
        ReferenceName(s.to_owned())
    }
}

impl From<String> for ReferenceName {
    fn from(s: String) -> Self {
        ReferenceName(s)
    }
}

impl From<NonZeroOid> for ReferenceName {
    fn from(oid: NonZeroOid) -> Self {
        Self::from(oid.to_string())
    }
}

impl From<MaybeZeroOid> for ReferenceName {
    fn from(oid: MaybeZeroOid) -> Self {
        Self::from(oid.to_string())
    }
}

impl AsRef<str> for ReferenceName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Represents a reference to an object.
pub struct Reference<'repo> {
    pub(super) inner: git2::Reference<'repo>,
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

    /// Get the name of this reference.
    #[instrument]
    pub fn get_name(&self) -> Result<ReferenceName> {
        let name = ReferenceName::from_bytes(self.inner.name_bytes().to_vec())?;
        Ok(name)
    }
    /// Get the commit object pointed to by this reference. Returns `None` if
    /// the object pointed to by the reference is a different kind of object.
    #[instrument]
    pub fn peel_to_commit(&self) -> Result<Option<Commit<'repo>>> {
        let object = match self.inner.peel(git2::ObjectType::Commit) {
            Ok(object) => object,
            Err(err) if err.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(err) => return Err(Error::ResolveReference(err)),
        };
        match object.into_commit() {
            Ok(commit) => Ok(Some(Commit { inner: commit })),
            Err(_) => Ok(None),
        }
    }

    /// Delete the reference.
    #[instrument]
    pub fn delete(&mut self) -> Result<()> {
        self.inner.delete().map_err(Error::DeleteReference)?;
        Ok(())
    }
}

/// Determine what kind of branch a reference is, given its name. The returned
/// `suffix` value is converted to a `String` to be rendered to the screen, so
/// it may have lost some information if the reference name had unusual
/// characters.
///
/// FIXME: This abstraction seems uncomfortable and clunky to use; consider
/// revising.
#[derive(Debug)]
pub enum CategorizedReferenceName<'a> {
    /// The reference represents a local branch.
    LocalBranch {
        /// The full name of the reference.
        name: &'a str,

        /// The string `refs/heads/`.
        prefix: &'static str,
    },

    /// The reference represents a remote branch.
    RemoteBranch {
        /// The full name of the reference.
        name: &'a str,

        /// The string `refs/remotes/`.
        prefix: &'static str,
    },

    /// Some other kind of reference which isn't a branch at all.
    OtherRef {
        /// The full name of the reference.
        name: &'a str,
    },
}

impl<'a> CategorizedReferenceName<'a> {
    /// Categorize the provided reference name.
    pub fn new(name: &'a ReferenceName) -> Self {
        let name = name.as_str();
        if name.starts_with("refs/heads/") {
            Self::LocalBranch {
                name,
                prefix: "refs/heads/",
            }
        } else if name.starts_with("refs/remotes/") {
            Self::RemoteBranch {
                name,
                prefix: "refs/remotes/",
            }
        } else {
            Self::OtherRef { name }
        }
    }

    /// Remove the prefix from the reference name. May raise an error if the
    /// result couldn't be encoded as an `String` (shouldn't happen).
    #[instrument]
    pub fn remove_prefix(&self) -> Result<String> {
        let (name, prefix): (_, &'static str) = match self {
            Self::LocalBranch { name, prefix } => (name, prefix),
            Self::RemoteBranch { name, prefix } => (name, prefix),
            Self::OtherRef { name } => (name, ""),
        };
        Ok(name.strip_prefix(prefix).unwrap_or(name).to_owned())
    }

    /// Render the full name of the reference, including its prefix, lossily as
    /// a `String`.
    pub fn render_full(&self) -> String {
        let name = match self {
            Self::LocalBranch { name, prefix: _ } => name,
            Self::RemoteBranch { name, prefix: _ } => name,
            Self::OtherRef { name } => name,
        };
        (*name).to_owned()
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
        name.strip_prefix(prefix).unwrap_or(name).to_owned()
    }

    /// Render the reference name lossily, and prepend a helpful string like
    /// `branch` to the description.
    pub fn friendly_describe(&self) -> String {
        let name = self.render_suffix();
        match self {
            CategorizedReferenceName::LocalBranch { .. } => {
                format!("branch {}", name)
            }
            CategorizedReferenceName::RemoteBranch { .. } => {
                format!("remote branch {}", name)
            }
            CategorizedReferenceName::OtherRef { .. } => format!("ref {}", name),
        }
    }
}

/// Re-export of [`git2::BranchType`]. This might change to be an opaque type later.
pub type BranchType = git2::BranchType;

/// Represents a Git branch.
pub struct Branch<'repo> {
    pub(super) repo: &'repo Repo,
    pub(super) inner: git2::Branch<'repo>,
}

impl std::fmt::Debug for Branch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "<Branch name={:?}>",
            String::from_utf8_lossy(
                self.inner
                    .name_bytes()
                    .unwrap_or(b"(could not get branch name)")
            ),
        )
    }
}

impl<'repo> Branch<'repo> {
    /// Get the OID pointed to by the branch. Returns `None` if the branch is
    /// not a direct reference (which is unusual).
    pub fn get_oid(&self) -> Result<Option<NonZeroOid>> {
        Ok(self.inner.get().target().map(make_non_zero_oid))
    }

    /// Get the name of this branch, not including any `refs/heads/` prefix. To get the full
    /// reference name of this branch, instead call `.into_reference().get_name()?`.
    #[instrument]
    pub fn get_name(&self) -> eyre::Result<&str> {
        self.inner
            .name()?
            .ok_or_else(|| eyre::eyre!("Could not decode branch name"))
    }

    /// Get the full reference name of this branch, including the `refs/heads/` or `refs/remotes/`
    /// prefix, as appropriate
    #[instrument]
    pub fn get_reference_name(&self) -> eyre::Result<ReferenceName> {
        let reference_name = self
            .inner
            .get()
            .name()
            .ok_or_else(|| eyre::eyre!("Could not decode branch reference name"))?;
        Ok(ReferenceName(reference_name.to_owned()))
    }

    /// If this branch tracks a remote ("upstream") branch, return that branch.
    #[instrument]
    pub fn get_upstream_branch(&self) -> Result<Option<Branch<'repo>>> {
        match self.inner.upstream() {
            Ok(upstream) => Ok(Some(Branch {
                repo: self.repo,
                inner: upstream,
            })),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => {
                let branch_name = self.inner.name_bytes().map_err(|_err| Error::DecodeUtf8 {
                    item: "branch name",
                })?;
                Err(Error::FindUpstreamBranch {
                    source: err,
                    name: String::from_utf8_lossy(branch_name).into_owned(),
                })
            }
        }
    }

    /// If this branch tracks a remote ("upstream") branch, return the OID of the commit which that
    /// branch points to.
    #[instrument]
    pub fn get_upstream_branch_target(&self) -> eyre::Result<Option<NonZeroOid>> {
        let upstream_branch = match self.get_upstream_branch()? {
            Some(upstream_branch) => upstream_branch,
            None => return Ok(None),
        };
        let target_oid = upstream_branch.get_oid()?;
        Ok(target_oid)
    }

    /// Get the associated remote to push to for this branch. If there is no
    /// associated remote, returns `None`. Note that this never reads the value
    /// of `push.remoteDefault`.
    #[instrument]
    pub fn get_push_remote_name(&self) -> eyre::Result<Option<String>> {
        let branch_name = self
            .inner
            .name()?
            .ok_or_else(|| eyre::eyre!("Branch name was not UTF-8: {self:?}"))?;
        let config = self.repo.get_readonly_config()?;
        if let Some(remote_name) = config.get(format!("branch.{branch_name}.pushRemote"))? {
            Ok(Some(remote_name))
        } else if let Some(remote_name) = config.get(format!("branch.{branch_name}.remote"))? {
            Ok(Some(remote_name))
        } else {
            Ok(None)
        }
    }

    /// Convert the branch into its underlying `Reference`.
    pub fn into_reference(self) -> Reference<'repo> {
        Reference {
            inner: self.inner.into_reference(),
        }
    }
}
