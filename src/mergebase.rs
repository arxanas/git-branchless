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
use anyhow::Context;
use pyo3::prelude::*;
use pyo3::types::PyTuple;
use rusqlite::OptionalExtension;

use crate::python::{get_conn, map_err_to_py_err};
use crate::util::wrap_git_error;

struct MergeBaseDb {
    conn: rusqlite::Connection,
}

fn init_tables(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    conn.execute(
        "
CREATE TABLE IF NOT EXISTS merge_base_oids (
    lhs_oid TEXT NOT NULL,
    rhs_oid TEXT NOT NULL,
    merge_base_oid TEXT,
    UNIQUE (lhs_oid, rhs_oid)
)
",
        rusqlite::params![],
    )
    .context("Creating tables")?;
    Ok(())
}

impl MergeBaseDb {
    fn new(conn: rusqlite::Connection) -> anyhow::Result<Self> {
        init_tables(&conn).context("Initializing tables")?;
        Ok(MergeBaseDb { conn })
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
    ) -> anyhow::Result<Option<git2::Oid>> {
        let (lhs_oid, rhs_oid) = if lhs_oid < rhs_oid {
            (lhs_oid, rhs_oid)
        } else {
            (rhs_oid, lhs_oid)
        };

        let merge_base_oid: Option<Option<String>> = self
            .conn
            .query_row_named(
                "
SELECT merge_base_oid
FROM merge_base_oids
WHERE lhs_oid = :lhs_oid
  AND rhs_oid = :rhs_oid
",
                rusqlite::named_params! {
                    ":lhs_oid": lhs_oid.to_string(),
                    ":rhs_oid": rhs_oid.to_string(),
                },
                |row| row.get("merge_base_oid"),
            )
            .optional()
            .context("Querying merge-base DB")?;

        match merge_base_oid {
            // Cached and non-NULL.
            Some(Some(merge_base_oid)) => {
                let merge_base_oid =
                    git2::Oid::from_str(&merge_base_oid).context("Parsing merge-base OID")?;
                Ok(Some(merge_base_oid))
            }

            // Cached and NULL.
            Some(None) => Ok(None),

            // Not cached.
            None => {
                let merge_base_oid = match repo.merge_base(lhs_oid, rhs_oid) {
                    Ok(merge_base_oid) => Ok(Some(merge_base_oid)),
                    Err(err) => {
                        if err.code() == git2::ErrorCode::NotFound {
                            Ok(None)
                        } else {
                            Err(wrap_git_error(err))
                        }
                    }
                }
                .context("Querying Git repository for merge-base OID")?;

                // Cache computed merge-base OID.
                self.conn
                    .execute_named(
                        "
INSERT INTO merge_base_oids VALUES (
    :lhs_oid,
    :rhs_oid,
    :merge_base_oid
)",
                        rusqlite::named_params! {
                            ":lhs_oid": &lhs_oid.to_string(),
                            ":rhs_oid": &rhs_oid.to_string(),
                            ":merge_base_oid": &merge_base_oid.map(|oid| oid.to_string()),
                        },
                    )
                    .context("Caching merge-base OID")?;

                Ok(merge_base_oid)
            }
        }
    }
}

#[pyclass]
pub struct PyMergeBaseDb {
    merge_base_db: MergeBaseDb,
}

#[pymethods]
impl PyMergeBaseDb {
    #[new]
    fn new(py: Python, conn: PyObject) -> PyResult<Self> {
        let conn = get_conn(py, conn)?;
        let merge_base_db = MergeBaseDb::new(conn).context("Constructing merge-base DB");
        let merge_base_db = map_err_to_py_err(
            merge_base_db,
            String::from("Could not construct merge-base database"),
        )?;

        let merge_base_db = PyMergeBaseDb { merge_base_db };
        Ok(merge_base_db)
    }

    fn get_merge_base_oid(
        &self,
        py: Python,
        repo: PyObject,
        lhs_oid: PyObject,
        rhs_oid: PyObject,
    ) -> PyResult<PyObject> {
        let repo_path: String = repo.getattr(py, "path")?.extract(py)?;
        let py_repo = repo;
        let repo = git2::Repository::open(repo_path);
        let repo = map_err_to_py_err(repo, String::from("Could not open Git repo"))?;

        let lhs_oid: String = lhs_oid.getattr(py, "hex")?.extract(py)?;
        let lhs_oid = git2::Oid::from_str(&lhs_oid);
        let lhs_oid = map_err_to_py_err(lhs_oid, String::from("Could not process LHS OID"))?;

        let rhs_oid: String = rhs_oid.getattr(py, "hex")?.extract(py)?;
        let rhs_oid = git2::Oid::from_str(&rhs_oid);
        let rhs_oid = map_err_to_py_err(rhs_oid, String::from("Could not process RHS OID"))?;

        let merge_base_oid = self
            .merge_base_db
            .get_merge_base_oid(repo, lhs_oid, rhs_oid);
        let merge_base_oid =
            map_err_to_py_err(merge_base_oid, String::from("Could not get merge base OID"))?;
        match merge_base_oid {
            Some(merge_base_oid) => {
                let args = PyTuple::new(py, &[merge_base_oid.to_string()]);
                let merge_base_commit = py_repo.call_method1(py, "__getitem__", args)?;
                let merge_base_oid = merge_base_commit.getattr(py, "oid")?;
                Ok(merge_base_oid)
            }
            None => Ok(Python::None(py)),
        }
    }
}
