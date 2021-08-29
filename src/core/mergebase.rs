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

use std::collections::VecDeque;

use eyre::Context;
use rusqlite::OptionalExtension;
use tracing::instrument;

use crate::core::eventlog::EventReplayer;
use crate::git::{Commit, NonZeroOid, Repo};
use crate::tui::{Effects, OperationType};

/// Service that can answer merge-base queries.
pub trait MergeBaseDb: std::fmt::Debug {
    /// Get an arbitrary merge-base between two commits.
    fn get_merge_base_oid(
        &self,
        effects: &Effects,
        repo: &Repo,
        lhs_oid: NonZeroOid,
        rhs_oid: NonZeroOid,
    ) -> eyre::Result<Option<NonZeroOid>>;

    /// Find a shortest path between the given commits.
    ///
    /// This is particularly important for multi-parent commits (i.e. merge commits).
    /// If we don't happen to traverse the correct parent, we may end up traversing a
    /// huge amount of commit history, with a significant performance hit.
    ///
    /// Args:
    /// * `repo`: The Git repository.
    /// * `commit_oid`: The OID of the commit to start at. We take parents of the
    /// provided commit until we end up at the target OID.
    /// * `target_oid`: The OID of the commit to end at.
    ///
    /// Returns: A path of commits from `commit_oid` through parents to `target_oid`.
    /// The path includes `commit_oid` at the beginning and `target_oid` at the end.
    /// If there is no such path, returns `None`.
    fn find_path_to_merge_base<'repo>(
        &self,
        effects: &Effects,
        repo: &'repo Repo,
        commit_oid: NonZeroOid,
        target_oid: NonZeroOid,
    ) -> eyre::Result<Option<Vec<Commit<'repo>>>>;
}

/// On-disk cache for merge-base queries.
pub struct SqliteMergeBaseDb<'conn> {
    conn: &'conn rusqlite::Connection,
}

impl std::fmt::Debug for SqliteMergeBaseDb<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<SqliteMergeBaseDb>")
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

impl<'conn> SqliteMergeBaseDb<'conn> {
    /// Constructor.
    #[instrument]
    fn new(conn: &'conn rusqlite::Connection) -> eyre::Result<Self> {
        init_tables(conn).wrap_err("Initializing tables")?;
        Ok(SqliteMergeBaseDb { conn })
    }
}

fn find_path_to_merge_base_internal<'repo>(
    effects: &Effects,
    repo: &'repo Repo,
    merge_base_db: &impl MergeBaseDb,
    commit_oid: NonZeroOid,
    target_oid: NonZeroOid,
    mut visited_commit_callback: impl FnMut(NonZeroOid),
) -> eyre::Result<Option<Vec<Commit<'repo>>>> {
    let (effects, _progress) = effects.start_operation(OperationType::FindPathToMergeBase);

    let mut queue = VecDeque::new();
    visited_commit_callback(commit_oid);
    let first_commit = match repo.find_commit(commit_oid)? {
        Some(commit) => commit,
        None => eyre::bail!("Unable to find commit with OID: {:?}", commit_oid),
    };
    queue.push_back(vec![first_commit]);
    let merge_base_oid =
        merge_base_db.get_merge_base_oid(&effects, repo, commit_oid, target_oid)?;
    while let Some(path) = queue.pop_front() {
        let last_commit = path
            .last()
            .expect("find_path_to_merge_base: empty path in queue");
        if last_commit.get_oid() == target_oid {
            return Ok(Some(path));
        }
        if Some(last_commit.get_oid()) == merge_base_oid {
            // We've hit the common ancestor of these two commits without
            // finding a path between them. That means it's impossible to find a
            // path between them by traversing more ancestors. Possibly the
            // caller passed them in in the wrong order, i.e. `commit_oid` is
            // actually a parent of `target_oid`.
            continue;
        }

        for parent in last_commit.get_parents() {
            visited_commit_callback(parent.get_oid());
            let mut new_path = path.clone();
            new_path.push(parent);
            queue.push_back(new_path);
        }
    }
    Ok(None)
}

impl MergeBaseDb for SqliteMergeBaseDb<'_> {
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
    fn get_merge_base_oid(
        &self,
        effects: &Effects,
        repo: &Repo,
        lhs_oid: NonZeroOid,
        rhs_oid: NonZeroOid,
    ) -> eyre::Result<Option<NonZeroOid>> {
        let (_effects, _progress) =
            effects.start_operation(crate::tui::OperationType::GetMergeBase);

        let (lhs_oid, rhs_oid) = if lhs_oid < rhs_oid {
            (lhs_oid, rhs_oid)
        } else {
            (rhs_oid, lhs_oid)
        };

        let merge_base_oid: Option<Option<String>> = self
            .conn
            .query_row(
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
                    .execute(
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

    #[instrument]
    fn find_path_to_merge_base<'repo>(
        &self,
        effects: &Effects,
        repo: &'repo Repo,
        commit_oid: NonZeroOid,
        target_oid: NonZeroOid,
    ) -> eyre::Result<Option<Vec<Commit<'repo>>>> {
        find_path_to_merge_base_internal(effects, repo, self, commit_oid, target_oid, |_commit| {})
    }
}

/// Instantiate a `MergeBaseDb` based on the requested compile-time feature.
#[cfg(feature = "eden-dag")]
pub fn make_merge_base_db(
    effects: &Effects,
    repo: &Repo,
    _conn: &rusqlite::Connection,
    event_replayer: &EventReplayer,
) -> eyre::Result<crate::git::Dag> {
    crate::git::Dag::open(effects, repo, event_replayer)
}

/// Instantiate a `MergeBaseDb` based on the requested compile-time feature.
#[cfg(not(feature = "eden-dag"))]
pub fn make_merge_base_db<'conn>(
    _effects: &Effects,
    _repo: &Repo,
    conn: &'conn rusqlite::Connection,
    _event_replayer: &EventReplayer,
) -> eyre::Result<SqliteMergeBaseDb<'conn>> {
    Ok(SqliteMergeBaseDb::new(conn)?)
}

#[cfg(test)]

mod tests {
    use super::*;

    use std::collections::HashSet;

    use crate::core::eventlog::EventLogDb;
    use crate::core::formatting::Glyphs;
    use crate::core::mergebase::make_merge_base_db;
    use crate::testing::make_git;

    #[test]
    fn test_find_path_to_merge_base_stop_early() -> eyre::Result<()> {
        let git = make_git()?;

        git.init_repo()?;
        let test1_oid = git.commit_file("test1", 1)?;
        let test2_oid = git.commit_file("test2", 2)?;
        git.detach_head()?;
        let test3_oid = git.commit_file("test3", 3)?;

        let mut effects = Effects::new_suppress_for_test(Glyphs::detect());
        let repo = git.get_repo()?;
        let conn = repo.get_db_conn()?;
        let event_log_db = EventLogDb::new(&conn)?;
        let event_replayer = EventReplayer::from_event_log_db(&effects, &repo, &event_log_db)?;
        let merge_base_db = make_merge_base_db(&effects, &repo, &conn, &event_replayer)?;

        let mut seen_oids = HashSet::new();
        let path = find_path_to_merge_base_internal(
            &mut effects,
            &repo,
            &merge_base_db,
            test2_oid,
            test3_oid,
            |oid| {
                seen_oids.insert(oid);
            },
        )?;
        assert!(path.is_none());

        assert!(seen_oids.contains(&test2_oid));
        assert!(!seen_oids.contains(&test3_oid));
        assert!(!seen_oids.contains(&test1_oid));

        Ok(())
    }
}
