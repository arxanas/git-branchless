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

use eyre::Context;
use rusqlite::OptionalExtension;
use tracing::instrument;

use crate::git::{NonZeroOid, Repo};
use crate::tui::Output;

/// On-disk cache for merge-base queries.
pub struct MergeBaseDb<'conn> {
    conn: &'conn rusqlite::Connection,
}

impl std::fmt::Debug for MergeBaseDb<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<MergeBaseDb>")
    }
}

#[instrument]
fn init_tables(conn: &rusqlite::Connection) -> eyre::Result<()> {
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
    .wrap_err("Creating tables")?;
    Ok(())
}

impl<'conn> MergeBaseDb<'conn> {
    /// Constructor.
    #[instrument]
    pub fn new(conn: &'conn rusqlite::Connection) -> eyre::Result<Self> {
        init_tables(&conn).wrap_err("Initializing tables")?;
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
    #[instrument]
    pub fn get_merge_base_oid(
        &self,
        output: &Output,
        repo: &Repo,
        lhs_oid: NonZeroOid,
        rhs_oid: NonZeroOid,
    ) -> eyre::Result<Option<NonZeroOid>> {
        let _progress = output.start_operation(crate::tui::OperationType::GetMergeBase);

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
            .wrap_err("Querying merge-base DB")?;

        match merge_base_oid {
            // Cached and non-NULL.
            Some(Some(merge_base_oid)) => {
                let merge_base_oid: NonZeroOid =
                    merge_base_oid.parse().wrap_err("Parsing merge-base OID")?;
                Ok(Some(merge_base_oid))
            }

            // Cached and NULL.
            Some(None) => Ok(None),

            // Not cached.
            None => {
                let merge_base_oid = repo.find_merge_base(lhs_oid, rhs_oid)?;

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
                    .wrap_err("Caching merge-base OID")?;

                Ok(merge_base_oid)
            }
        }
    }
}
