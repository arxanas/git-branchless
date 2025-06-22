//! Utilities to fetch, confirm and save a list of untracked files, so we can
//! prompt the user about them.

use console::{Key, Term};
use eyre::Context;
use std::io::Write as IoWrite;
use std::time::SystemTime;
use std::{collections::HashSet, fmt::Write};
use tracing::instrument;

use crate::git::{GitRunInfo, Repo};

use super::{effects::Effects, eventlog::EventTransactionId, formatting::Pluralize};

/// TODO
#[instrument]
pub fn prompt_about_untracked_files(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_tx_id: EventTransactionId,
) -> eyre::Result<Vec<String>> {
    let conn = repo.get_db_conn()?;

    let cached_files = get_cached_untracked_files(&conn)?;
    let real_files = get_real_untracked_files(repo, event_tx_id, git_run_info)?;
    let new_files: Vec<&String> = real_files.difference(&cached_files).collect();

    let mut files_to_add = vec![];
    if !new_files.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "Found {}:",
            Pluralize {
                determiner: None,
                amount: new_files.len(),
                unit: ("new untracked file", "new untracked files"),
            },
        )?;
        'outer: for file in new_files {
            write!(
                effects.get_output_stream(),
                "  Add file '{file}'? [Yes/(N)o/nOne] "
            )?;
            std::io::stdout().flush()?;

            let term = Term::stderr();
            'inner: loop {
                let key = term.read_key()?;
                match key {
                    Key::Char('y') | Key::Char('Y') => {
                        files_to_add.push(file.clone());
                        writeln!(effects.get_output_stream(), "adding")?;
                    }
                    Key::Char('n') | Key::Char('N') | Key::Enter => {
                        writeln!(effects.get_output_stream(), "not adding")?;
                    }
                    Key::Char('o') | Key::Char('O') => {
                        writeln!(effects.get_output_stream(), "skipping remaining")?;
                        break 'outer;
                    }
                    _ => continue 'inner,
                };
                continue 'outer;
            }
        }
    }

    cache_untracked_files(&conn, real_files)?;

    Ok(files_to_add)
}

/// TODO
#[instrument]
fn get_real_untracked_files(
    repo: &Repo,
    event_tx_id: EventTransactionId,
    git_run_info: &GitRunInfo,
) -> eyre::Result<HashSet<String>> {
    let args = vec!["ls-files", "--others", "--exclude-standard", "-z"];
    let files_str = git_run_info
        .run_silent(repo, Some(event_tx_id), &args, Default::default())
        .wrap_err("calling `git ls-files`")?
        .stdout;
    let files_str = String::from_utf8(files_str).wrap_err("Decoding stdout from Git subprocess")?;
    let files = files_str
        .trim()
        .split('\0')
        .filter_map(|s| {
            if s.is_empty() {
                None
            } else {
                Some(s.to_owned())
            }
        })
        .collect();
    Ok(files)
}

/// TODO
#[instrument]
fn cache_untracked_files(conn: &rusqlite::Connection, files: HashSet<String>) -> eyre::Result<()> {
    {
        conn.execute("DROP TABLE IF EXISTS untracked_files", rusqlite::params![])
            .wrap_err("Removing `untracked_files` table")?;
    }

    init_untracked_files_table(conn)?;

    {
        let tx = conn.unchecked_transaction()?;

        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .wrap_err("Calculating event transaction timestamp")?
            .as_secs_f64();
        for file in files {
            tx.execute(
                "
                INSERT INTO untracked_files
                    (timestamp, file)
                VALUES
                    (:timestamp, :file)
                ",
                rusqlite::named_params! {
                    ":timestamp": timestamp,
                    ":file": file,
                },
            )?;
        }
        tx.commit()?;
    }

    Ok(())
}

/// Ensure the untracked_files table exists; creating it if it does not.
#[instrument]
fn init_untracked_files_table(conn: &rusqlite::Connection) -> eyre::Result<()> {
    conn.execute(
        "
        CREATE TABLE IF NOT EXISTS untracked_files (
            timestamp REAL NOT NULL,
            file TEXT NOT NULL
        )
        ",
        rusqlite::params![],
    )
    .wrap_err("Creating `untracked_files` table")?;

    Ok(())
}

/// TODO
#[instrument]
pub fn get_cached_untracked_files(conn: &rusqlite::Connection) -> eyre::Result<HashSet<String>> {
    init_untracked_files_table(conn)?;

    let mut stmt = conn.prepare("SELECT file FROM untracked_files")?;
    let paths = stmt
        .query_map(rusqlite::named_params![], |row| row.get("file"))?
        .filter_map(|p| p.ok())
        .collect();
    Ok(paths)
}
