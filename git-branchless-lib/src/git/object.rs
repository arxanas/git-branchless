use std::path::Path;

use bstr::{BString, ByteSlice};
use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;
use git2::message_trailers_bytes;
use tracing::instrument;

use crate::core::formatting::{Glyphs, StyledStringBuilder};
use crate::core::node_descriptors::{
    render_node_descriptors, CommitMessageDescriptor, CommitOidDescriptor, NodeObject, Redactor,
};
use crate::git::oid::make_non_zero_oid;
use crate::git::repo::{Error, Result, Signature};
use crate::git::{NonZeroOid, Time, Tree};

use super::MaybeZeroOid;

/// Represents a commit object in the Git object database.
#[derive(Clone, Debug)]
pub struct Commit<'repo> {
    pub(super) inner: git2::Commit<'repo>,
}

impl<'repo> Commit<'repo> {
    /// Get the object ID of the commit.
    #[instrument]
    pub fn get_oid(&self) -> NonZeroOid {
        NonZeroOid {
            inner: self.inner.id(),
        }
    }

    /// Get the short object ID of the commit.
    #[instrument]
    pub fn get_short_oid(&self) -> Result<String> {
        Ok(String::from_utf8_lossy(
            &self
                .inner
                .clone()
                .into_object()
                .short_id()
                .map_err(Error::Git)?,
        )
        .to_string())
    }

    /// Get the object IDs of the parents of this commit.
    #[instrument]
    pub fn get_parent_oids(&self) -> Vec<NonZeroOid> {
        self.inner.parent_ids().map(make_non_zero_oid).collect()
    }

    /// Get the parent OID of this commit if there is exactly one parent, or
    /// `None` otherwise.
    #[instrument]
    pub fn get_only_parent_oid(&self) -> Option<NonZeroOid> {
        match self.get_parent_oids().as_slice() {
            [] | [_, _, ..] => None,
            [only_parent_oid] => Some(*only_parent_oid),
        }
    }

    /// Get the number of parents of this commit.
    #[instrument]
    pub fn get_parent_count(&self) -> usize {
        self.inner.parent_count()
    }

    /// Get the parent commits of this commit.
    #[instrument]
    pub fn get_parents(&self) -> Vec<Commit<'repo>> {
        self.inner
            .parents()
            .map(|commit| Commit { inner: commit })
            .collect()
    }

    /// Get the parent of this commit if there is exactly one parent, or `None`
    /// otherwise.
    #[instrument]
    pub fn get_only_parent(&self) -> Option<Commit<'repo>> {
        match self.get_parents().as_slice() {
            [] | [_, _, ..] => None,
            [only_parent] => Some(only_parent.clone()),
        }
    }

    /// Get the commit time of this commit.
    #[instrument]
    pub fn get_time(&self) -> Time {
        Time {
            inner: self.inner.time(),
        }
    }

    /// Get the summary (first line) of the commit message.
    #[instrument]
    pub fn get_summary(&self) -> Result<BString> {
        match self.inner.summary_bytes() {
            Some(summary) => Ok(BString::from(summary)),
            None => Err(Error::DecodeUtf8 { item: "summary" }),
        }
    }

    /// Get the commit message with some whitespace trimmed.
    #[instrument]
    pub fn get_message_pretty(&self) -> BString {
        BString::from(self.inner.message_bytes())
    }

    /// Get the commit message, without any whitespace trimmed.
    #[instrument]
    pub fn get_message_raw(&self) -> BString {
        BString::from(self.inner.message_raw_bytes())
    }

    /// Get the author of this commit.
    #[instrument]
    pub fn get_author(&self) -> Signature<'_> {
        Signature {
            inner: self.inner.author(),
        }
    }

    /// Get the committer of this commit.
    #[instrument]
    pub fn get_committer(&self) -> Signature<'_> {
        Signature {
            inner: self.inner.committer(),
        }
    }

    /// Get the OID of the `Tree` object associated with this commit.
    #[instrument]
    pub fn get_tree_oid(&self) -> MaybeZeroOid {
        self.inner.tree_id().into()
    }

    /// Get the `Tree` object associated with this commit.
    #[instrument]
    pub fn get_tree(&self) -> Result<Tree<'_>> {
        let tree = self.inner.tree().map_err(|err| Error::FindTree {
            source: err,
            oid: self.inner.tree_id().into(),
        })?;
        Ok(Tree { inner: tree })
    }

    /// Get the "trailer" metadata from this commit's message. These are strings
    /// like `Signed-off-by: foo` which appear at the end of the commit message.
    #[instrument]
    pub fn get_trailers(&self) -> Result<Vec<(String, String)>> {
        let message = self.get_message_raw();
        let message = message.to_str().map_err(|_| Error::DecodeUtf8 {
            item: "raw message",
        })?;
        let mut result = Vec::new();
        for (k, v) in message_trailers_bytes(message)
            .map_err(Error::ReadMessageTrailer)?
            .iter()
        {
            if let (Ok(k), Ok(v)) = (std::str::from_utf8(k), std::str::from_utf8(v)) {
                result.push((k.to_owned(), v.to_owned()));
            }
        }
        Ok(result)
    }

    /// Print a one-line description of this commit containing its OID and
    /// summary.
    #[instrument]
    pub fn friendly_describe(&self, glyphs: &Glyphs) -> Result<StyledString> {
        let description = render_node_descriptors(
            glyphs,
            &NodeObject::Commit {
                commit: self.clone(),
            },
            &mut [
                &mut CommitOidDescriptor::new(true).map_err(|err| Error::DescribeCommit {
                    source: err,
                    commit: self.get_oid(),
                })?,
                &mut CommitMessageDescriptor::new(&Redactor::Disabled).map_err(|err| {
                    Error::DescribeCommit {
                        source: err,
                        commit: self.get_oid(),
                    }
                })?,
            ],
        )
        .map_err(|err| Error::DescribeCommit {
            source: err,
            commit: self.get_oid(),
        })?;
        Ok(description)
    }

    /// Print a shortened colorized version of the OID of this commit.
    #[instrument]
    pub fn friendly_describe_oid(&self, glyphs: &Glyphs) -> Result<StyledString> {
        let description = render_node_descriptors(
            glyphs,
            &NodeObject::Commit {
                commit: self.clone(),
            },
            &mut [
                &mut CommitOidDescriptor::new(true).map_err(|err| Error::DescribeCommit {
                    source: err,
                    commit: self.get_oid(),
                })?,
            ],
        )
        .map_err(|err| Error::DescribeCommit {
            source: err,
            commit: self.get_oid(),
        })?;
        Ok(description)
    }

    /// Get a multi-line description of this commit containing information about
    /// its OID, author, commit time, and message.
    #[instrument]
    pub fn friendly_preview(&self) -> Result<StyledString> {
        let commit_time = self.get_time().to_date_time();
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
            StyledString::styled(
                format!(
                    "Date:\t{}",
                    commit_time
                        .map(|commit_time| commit_time.to_string())
                        .unwrap_or_else(|| "?".to_string())
                ),
                BaseColor::Green.light(),
            ),
            StyledString::plain(textwrap::indent(
                &self.get_message_pretty().to_str_lossy(),
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
    #[instrument]
    pub fn contains_touched_path(&self, path: &Path) -> Result<Option<bool>> {
        let parent = match self.get_only_parent() {
            None => return Ok(None),
            Some(parent) => parent,
        };
        let parent_tree = parent.get_tree()?;
        let current_tree = self.get_tree()?;
        let parent_oid = parent_tree
            .get_oid_for_path(path)
            .map_err(Error::ReadTreeEntry)?;
        let current_oid = current_tree
            .get_oid_for_path(path)
            .map_err(Error::ReadTreeEntry)?;
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
    ) -> Result<NonZeroOid> {
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
            .map_err(Error::Amend)?;
        Ok(make_non_zero_oid(oid))
    }
}

pub struct Blob<'repo> {
    pub(super) inner: git2::Blob<'repo>,
}

impl Blob<'_> {
    /// Get the size of the blob in bytes.
    pub fn size(&self) -> u64 {
        self.inner.size().try_into().unwrap()
    }

    /// Get the content of the blob as a byte slice.
    pub fn get_content(&self) -> &[u8] {
        self.inner.content()
    }

    /// Determine if the blob is binary. Note that this looks only at the
    /// content of the blob to determine if it's binary; attributes set in
    /// `.gitattributes`, etc. are not checked.
    pub fn is_binary(&self) -> bool {
        self.inner.is_binary()
    }
}
