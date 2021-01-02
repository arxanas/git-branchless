//! Persistent storage to cache merge-base queries.
//!
//! A "merge-base" can be described as the common ancestor of two commits.
//! Merge-bases are calculated to determine
//!
//!  1) Whether a commit is a branch off of the main branch.
//!  2) How to order two commits topologically.
//!
//! In a large repository, merge-base queries can be quite expensive when
//! comparing commits which are far away from each other. This can happen, for
//! example, whenever you do a `git pull` to update the main branch, but you
//! haven't yet updated any of your lines of work. Your lines of work are now far
//! away from the current main branch commit, so the merge-base calculation may
//! take a while. It can also happen when simply checking out an old commit to
//! examine it.

use pyo3::prelude::*;

struct MergeBaseDb {}

impl MergeBaseDb {
    fn new() -> Self {
        MergeBaseDb {}
    }

    /// Get the merge-base for two given commits.
    ///
    /// If the query is already in the cache, return the cached result. If
    /// not, it is computed, cached, and returned.
    ///
    /// Args:
    /// * `repo`: The Git repo.
    /// * `lhs_oid`: The first OID (ordering is arbitrary).
    /// * `rhs_oid`: The second OID (ordering is arbitrary).
    ///
    /// Returns: The merge-base OID for these two commits. Returns `None` if no
    /// merge-base could be found.
    fn get_merge_base_oid(
        &self,
        repo: git2::Repository,
        lhs_oid: git2::Oid,
        rhs_oid: git2::Oid,
    ) -> Option<git2::Oid> {
        None
    }
}

#[pyclass]
pub struct PyMergeBaseDb {
    merge_base_db: MergeBaseDb,
}

#[pymethods]
impl PyMergeBaseDb {
    #[new]
    fn new() -> Self {
        PyMergeBaseDb {
            merge_base_db: MergeBaseDb {},
        }
    }
}
