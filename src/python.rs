//! Helpers for the Rust/Python interop.

use anyhow::Context;
use rusqlite::NO_PARAMS;

/// HACK: Open a new SQLite connection to the same database.
///
/// This is for migration use only, because the connection cannot be shared
/// safely between threads. This is only a concern for accessing the connection
/// from Python. This function should be deleted once we no longer call from
/// Python.
pub fn clone_conn(conn: &rusqlite::Connection) -> anyhow::Result<rusqlite::Connection> {
    let db_path = conn
        .query_row("PRAGMA database_list", NO_PARAMS, |row| {
            let db_path: String = row.get(2)?;
            Ok(db_path)
        })
        .with_context(|| "Querying database list for cloning SQLite database")?;
    let conn = rusqlite::Connection::open(db_path)
        .with_context(|| "Opening cloned database connection")?;
    Ok(conn)
}
