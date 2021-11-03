//! Convenience commands to help the user move through a stack of commits.

use std::convert::TryInto;
use std::fmt::Write;

use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;
use eden_dag::DagAlgorithm;
use tracing::instrument;

use crate::commands::smartlog::make_smartlog_graph;
use crate::core::config::get_next_interactive;
use crate::core::effects::Effects;
use crate::core::eventlog::{EventLogDb, EventReplayer};
use crate::core::formatting::{printable_styled_string, Pluralize};
use crate::git::{check_out_commit, sort_commit_set, CommitSet, Dag, GitRunInfo, NonZeroOid, Repo};
use crate::opts::TraverseCommitsOptions;
use crate::tui::prompt_select_commit;

/// The command being invoked, indicating which direction to traverse commits.
#[derive(Clone, Copy, Debug)]
pub enum Command {
    /// Traverse child commits.
    Next,

    /// Traverse parent commits.
    Prev,
}

/// The number of commits to traverse.
#[derive(Clone, Copy, Debug)]
pub enum Distance {
    /// Traverse this number of commits.
    NumCommits(usize),

    /// Traverse as many commits as possible.
    AllTheWay,
}

/// Some commits have multiple children, which makes `next` ambiguous. These
/// values disambiguate which child commit to go to, according to the committed
/// date.
#[derive(Clone, Copy, Debug)]
pub enum Towards {
    /// When encountering multiple children, select the newest one.
    Newest,

    /// When encountering multiple children, select the oldest one.
    Oldest,

    /// When encountering multiple children, interactively prompt for
    /// which one to advance to.
    Interactive,
}

#[instrument]
fn advance(
    effects: &Effects,
    repo: &Repo,
    dag: &Dag,
    current_oid: NonZeroOid,
    command: Command,
    distance: Distance,
    towards: Option<Towards>,
) -> eyre::Result<Option<NonZeroOid>> {
    let towards = match towards {
        Some(towards) => Some(towards),
        None => {
            if get_next_interactive(repo)? {
                Some(Towards::Interactive)
            } else {
                None
            }
        }
    };

    let public_commits = dag.query().ancestors(dag.main_branch_commit.clone())?;

    let glyphs = effects.get_glyphs();
    let mut current_oid = current_oid;
    let mut i = 0;
    loop {
        let candidate_commits = match command {
            Command::Next => {
                let children = dag
                    .query()
                    .children(CommitSet::from(current_oid))?
                    .difference(&dag.obsolete_commits);
                sort_commit_set(repo, dag, &children)?
            }

            Command::Prev => {
                let parents = dag.query().parents(CommitSet::from(current_oid))?;

                // The `--all` flag for `git prev` isn't useful if all it does
                // is take you to the root commit for the repository.  Instead,
                // we assume that the user wanted to get to the root commit for
                // their current *commit stack*. We filter out commits which
                // aren't part of the commit stack so that we stop early here.
                let parents = match distance {
                    Distance::AllTheWay => parents.difference(&public_commits),
                    Distance::NumCommits(_) => parents,
                };

                sort_commit_set(repo, dag, &parents)?
            }
        };

        match distance {
            Distance::NumCommits(num_commits) => {
                if i == num_commits {
                    break;
                }
            }

            Distance::AllTheWay => {
                if candidate_commits.is_empty() {
                    break;
                }
            }
        }

        let pluralize = match command {
            Command::Next => Pluralize {
                amount: i.try_into()?,
                plural: "children",
                singular: "child",
            },

            Command::Prev => Pluralize {
                amount: i.try_into()?,
                plural: "parents",
                singular: "parent",
            },
        };
        let header = format!(
            "Found multiple possible {} commits to go to after traversing {}:",
            pluralize.singular,
            pluralize.to_string(),
        );

        current_oid = match (towards, candidate_commits.as_slice()) {
            (_, []) => {
                writeln!(
                    effects.get_output_stream(),
                    "{}",
                    printable_styled_string(
                        glyphs,
                        StyledString::styled(
                            format!(
                                "No more {} commits to go to after traversing {}.",
                                pluralize.singular,
                                pluralize.to_string(),
                            ),
                            BaseColor::Yellow.light()
                        )
                    )?
                )?;

                if i == 0 {
                    // If we didn't succeed in traversing any commits, then
                    // treat the operation as a failure. Otherwise, assume that
                    // the user just meant to go as many commits as possible.
                    return Ok(None);
                } else {
                    break;
                }
            }

            (_, [only_child]) => only_child.get_oid(),
            (Some(Towards::Newest), [.., newest_child]) => newest_child.get_oid(),
            (Some(Towards::Oldest), [oldest_child, ..]) => oldest_child.get_oid(),
            (Some(Towards::Interactive), [_, _, ..]) => {
                match prompt_select_commit(candidate_commits, Some(&header))? {
                    Some(oid) => oid,
                    None => {
                        return Ok(None);
                    }
                }
            }
            (None, [_, _, ..]) => {
                writeln!(effects.get_output_stream(), "{}", header)?;
                for (j, child) in (0..).zip(candidate_commits.iter()) {
                    let descriptor = if j == 0 {
                        " (oldest)"
                    } else if j + 1 == candidate_commits.len() {
                        " (newest)"
                    } else {
                        ""
                    };

                    writeln!(
                        effects.get_output_stream(),
                        "  {} {}{}",
                        glyphs.bullet_point,
                        printable_styled_string(glyphs, child.friendly_describe()?)?,
                        descriptor
                    )?;
                }
                writeln!(effects.get_output_stream(), "(Pass --oldest (-o), --newest (-n), or --interactive (-i) to select between ambiguous commits)")?;
                return Ok(None);
            }
        };

        i += 1;
    }
    Ok(Some(current_oid))
}

/// Go forward or backward a certain number of commits.
#[instrument]
pub fn traverse_commits(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    command: Command,
    options: &TraverseCommitsOptions,
) -> eyre::Result<isize> {
    let TraverseCommitsOptions {
        num_commits,
        all_the_way,
        oldest,
        newest,
        interactive,
    } = *options;

    let distance = match (all_the_way, num_commits) {
        (false, None) => Distance::NumCommits(1),
        (false, Some(num_commits)) => Distance::NumCommits(num_commits),
        (true, None) => Distance::AllTheWay,
        (true, Some(_)) => {
            eyre::bail!("num_commits and --all cannot both be set")
        }
    };

    let towards = match (oldest, newest, interactive) {
        (false, false, false) => None,
        (true, false, false) => Some(Towards::Oldest),
        (false, true, false) => Some(Towards::Newest),
        (false, false, true) => Some(Towards::Interactive),
        (_, _, _) => {
            eyre::bail!("Only one of --oldest, --newest, and --interactive can be set")
        }
    };

    let repo = Repo::from_current_dir()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let head_oid = match references_snapshot.head_oid {
        Some(head_oid) => head_oid,
        None => {
            eyre::bail!("No HEAD present; cannot calculate next commit");
        }
    };

    let current_oid = advance(effects, &repo, &dag, head_oid, command, distance, towards)?;
    let current_oid = match current_oid {
        None => return Ok(1),
        Some(current_oid) => current_oid,
    };

    check_out_commit(effects, git_run_info, None, &current_oid.to_string())
}

/// Interactively checkout a commit from the smartlog.
pub fn checkout(effects: &Effects, git_run_info: &GitRunInfo) -> eyre::Result<isize> {
    let repo = Repo::from_current_dir()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let graph = make_smartlog_graph(effects, &repo, &dag, &event_replayer, event_cursor, true)?;

    match prompt_select_commit(graph.get_commits(), None)? {
        Some(oid) => check_out_commit(effects, git_run_info, None, &oid.to_string()),
        None => Ok(1),
    }
}
