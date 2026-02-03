//! Utilities to fetch, confirm and save a list of untracked files, so we can
//! prompt the user about them.

use clap::ValueEnum;
use console::{Key, Term};
use cursive::theme::BaseColor;
use eyre::Context;
use itertools::Itertools;
use std::io::Write as IoWrite;
use std::time::SystemTime;
use std::{collections::HashSet, fmt::Write};
use tracing::instrument;

use super::{effects::Effects, eventlog::EventTransactionId, formatting::Pluralize};
use crate::core::config::{Hint, get_hint_enabled, get_hint_string, print_hint_suppression_notice};
use crate::core::formatting::StyledStringBuilder;
use crate::git::{ConfigRead, GitRunInfo, Repo};
use crate::util::{ExitCode, EyreExitOr};

/// How to handle untracked files when creating/amending commits.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum UntrackedFileStrategy {
    /// Add all untracked files.
    Add,
    /// Disable all untracked file checking and processing.
    Disable,
    /// Prompt the user about how to handle each untracked file.
    Prompt,
    /// Skip all untracked files.
    Skip,
}

/// Process untracked files according to the given or configured strategy.
/// Returns a list of files in the current repo that should be added to the
/// commit being processed by amend or record.
///
/// Note: may block while prompting for input, if such prompts are requested by
/// the strategy.
#[instrument]
pub fn process_untracked_files(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_tx_id: EventTransactionId,
    strategy: Option<UntrackedFileStrategy>,
) -> EyreExitOr<Vec<String>> {
    let conn = repo.get_db_conn()?;

    let strategy = match strategy {
        Some(strategy) => strategy,
        None => {
            let strategy_config_key = "branchless.record.untrackedFiles";
            let config = repo.get_readonly_config()?;
            let strategy: Option<String> = config.get(strategy_config_key)?;
            match strategy {
                None => UntrackedFileStrategy::Disable,
                Some(strategy) => match UntrackedFileStrategy::from_str(&strategy, true) {
                    Ok(strategy) => strategy,
                    Err(_) => {
                        writeln!(
                            effects.get_output_stream(),
                            "Invalid value for config value {strategy_config_key}: {strategy}"
                        )?;
                        writeln!(
                            effects.get_output_stream(),
                            "Expected one of: {}",
                            UntrackedFileStrategy::value_variants()
                                .iter()
                                .filter_map(|variant| variant.to_possible_value())
                                .map(|value| value.get_name().to_owned())
                                .join(", ")
                        )?;
                        return Ok(Err(ExitCode(1)));
                    }
                },
            }
        }
    };

    if let UntrackedFileStrategy::Disable = strategy {
        // earliest possible return to avoid hitting disk, db, etc
        return Ok(Ok(Vec::new()));
    }

    let cached_files = get_cached_untracked_files(&conn)?;
    let real_files = get_real_untracked_files(repo, event_tx_id, git_run_info)?;
    let new_files: Vec<String> = real_files
        .difference(&cached_files)
        .sorted()
        .cloned()
        .collect();
    let previously_skipped_files: Vec<String> =
        real_files.intersection(&cached_files).cloned().collect();

    cache_untracked_files(&conn, real_files)?;

    if !previously_skipped_files.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "Skipping {}: {}",
            Pluralize {
                determiner: None,
                amount: previously_skipped_files.len(),
                unit: ("previously skipped file", "previously skipped files"),
            },
            render_styled(effects, previously_skipped_files.join(", "),)
        )?;
    }

    if new_files.is_empty() {
        return Ok(Ok(Vec::new()));
    }

    let files_to_add = match strategy {
        UntrackedFileStrategy::Disable => unreachable!(),

        UntrackedFileStrategy::Add => {
            writeln!(
                effects.get_output_stream(),
                "Including {}: {}",
                Pluralize {
                    determiner: None,
                    amount: new_files.len(),
                    unit: ("new untracked file", "new untracked files"),
                },
                new_files.join(", ")
            )?;

            new_files
        }

        UntrackedFileStrategy::Skip => {
            writeln!(
                effects.get_output_stream(),
                "Skipping {}: {}",
                Pluralize {
                    determiner: None,
                    amount: new_files.len(),
                    unit: ("new untracked file", "new untracked files"),
                },
                render_styled(effects, new_files.join(", "),)
            )?;

            if get_hint_enabled(repo, Hint::AddSkippedFiles)? {
                writeln!(
                    effects.get_output_stream(),
                    "{}: {} will remain skipped and will not be automatically reconsidered",
                    effects.get_glyphs().render(get_hint_string())?,
                    if new_files.len() == 1 {
                        "this file"
                    } else {
                        "these files"
                    },
                )?;
                writeln!(
                    effects.get_output_stream(),
                    "{}: to add {} yourself: git add",
                    effects.get_glyphs().render(get_hint_string())?,
                    if new_files.len() == 1 { "it" } else { "them" },
                )?;
                print_hint_suppression_notice(effects, Hint::AddSkippedFiles)?;
            }

            Vec::new()
        }

        UntrackedFileStrategy::Prompt => {
            let mut files_to_add = Vec::new();
            let mut skip_remaining = false;
            writeln!(
                effects.get_output_stream(),
                "Found {}:",
                Pluralize {
                    determiner: None,
                    amount: new_files.len(),
                    unit: ("new untracked file", "new untracked files"),
                },
            )?;
            'file_loop: for file in new_files {
                if skip_remaining {
                    writeln!(effects.get_output_stream(), "  Skipping file '{file}'")?;
                    continue 'file_loop;
                }

                'prompt_loop: loop {
                    write!(
                        effects.get_output_stream(),
                        "  Include file '{file}'? {} ",
                        render_styled(effects, "[Yes/(N)o/nOne/Help]".to_string())
                    )?;
                    std::io::stdout().flush()?;

                    let term = Term::stderr();
                    'tty_input_loop: loop {
                        let key = term.read_key()?;
                        match key {
                            Key::Char('y') | Key::Char('Y') => {
                                files_to_add.push(file.clone());
                                writeln!(
                                    effects.get_output_stream(),
                                    "{}",
                                    render_styled(effects, "adding".to_string())
                                )?;
                            }

                            Key::Char('n') | Key::Char('N') | Key::Enter => {
                                writeln!(
                                    effects.get_output_stream(),
                                    "{}",
                                    render_styled(effects, "not adding".to_string())
                                )?;
                            }

                            Key::Char('o') | Key::Char('O') => {
                                skip_remaining = true;
                                writeln!(
                                    effects.get_output_stream(),
                                    "{}",
                                    render_styled(effects, "skipping remaining".to_string())
                                )?;
                            }

                            Key::Char('h') | Key::Char('H') | Key::Char('?') => {
                                writeln!(
                                    effects.get_output_stream(),
                                    "help\n\n\
                                     - y/Y: include the file\n\
                                     - n/N/<enter>: skip the file\n\
                                     - o/O: skip the file and all subsequent files\n\
                                     - h/H/?: show this help message\n\
                                    "
                                )?;
                                continue 'prompt_loop;
                            }

                            _ => continue 'tty_input_loop,
                        };
                        continue 'file_loop;
                    }
                }
            }

            files_to_add
        }
    };

    Ok(Ok(files_to_add))
}

fn render_styled(effects: &Effects, string_to_render: String) -> String {
    effects
        .get_glyphs()
        .render(
            StyledStringBuilder::new()
                .append_styled(string_to_render, BaseColor::Black.light())
                .build(),
        )
        .expect("rendering styled string")
}

/// Get a list of all untracked files that currently exist on disk.
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

/// Get a list of all untracked files that we have cached in the database. This
/// should be the list of all untracked files that existed on disk when we last
/// checked.
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

/// Persist a snapshot of existent, untracked files in the database.
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
