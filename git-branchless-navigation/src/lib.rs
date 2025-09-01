//! Convenience commands to help the user move through a stack of commits.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_conditions)]

pub mod prompt;

use std::collections::HashSet;

use std::ffi::OsString;
use std::fmt::Write;
use std::time::SystemTime;

use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;

use lib::core::check_out::{check_out_commit, CheckOutCommitOptions, CheckoutTarget};
use lib::core::repo_ext::RepoExt;
use lib::util::{ExitCode, EyreExitOr};
use tracing::{instrument, warn};

use git_branchless_opts::{ResolveRevsetOptions, Revset, SwitchOptions, TraverseCommitsOptions};
use git_branchless_revset::{resolve_commits, resolve_default_smartlog_commits};
use git_branchless_smartlog::make_smartlog_graph;
use lib::core::config::get_next_interactive;
use lib::core::dag::{sorted_commit_set, union_all, CommitSet, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::Pluralize;
use lib::core::node_descriptors::{
    BranchesDescriptor, CommitMessageDescriptor, CommitOidDescriptor,
    DifferentialRevisionDescriptor, NodeDescriptor, Redactor, RelativeTimeDescriptor,
};
use lib::git::{GitRunInfo, NonZeroOid, Repo};

use crate::prompt::prompt_select_commit;

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
    /// Traverse this number of commits or branches.
    NumCommits {
        /// The number of commits or branches to traverse.
        amount: usize,

        /// If `true`, count the number of branches traversed, not commits.
        move_by_branches: bool,
    },

    /// Traverse as many commits as possible.
    AllTheWay {
        /// If `true`, find the farthest commit with a branch attached to it.
        move_by_branches: bool,
    },
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

#[instrument(skip(commit_descriptors))]
fn advance(
    effects: &Effects,
    repo: &Repo,
    dag: &Dag,
    commit_descriptors: &mut [&mut dyn NodeDescriptor],
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

    let public_commits = dag.query_ancestors(dag.main_branch_commit.clone())?;

    let glyphs = effects.get_glyphs();
    let mut current_oid = current_oid;
    let mut i = 0;
    loop {
        let candidate_commits = match command {
            Command::Next => {
                let child_commits = || -> eyre::Result<CommitSet> {
                    let result = dag.query_children(CommitSet::from(current_oid))?;
                    let result = dag.filter_visible_commits(result)?;
                    Ok(result)
                };

                let descendant_branches = || -> eyre::Result<CommitSet> {
                    let descendant_commits = dag.query_descendants(child_commits()?)?;
                    let descendant_branches = dag.branch_commits.intersection(&descendant_commits);
                    let descendants = dag.query_descendants(descendant_branches)?;
                    let nearest_descendant_branches = dag.query_roots(descendants)?;
                    Ok(nearest_descendant_branches)
                };

                let children = match distance {
                    Distance::AllTheWay {
                        move_by_branches: false,
                    }
                    | Distance::NumCommits {
                        amount: _,
                        move_by_branches: false,
                    } => child_commits()?,

                    Distance::AllTheWay {
                        move_by_branches: true,
                    }
                    | Distance::NumCommits {
                        amount: _,
                        move_by_branches: true,
                    } => descendant_branches()?,
                };

                sorted_commit_set(repo, dag, &children)?
            }

            Command::Prev => {
                let parent_commits = || -> eyre::Result<CommitSet> {
                    let result = dag.query_parents(CommitSet::from(current_oid))?;
                    Ok(result)
                };
                let ancestor_branches = || -> eyre::Result<CommitSet> {
                    let ancestor_commits = dag.query_ancestors(parent_commits()?)?;
                    let ancestor_branches = dag.branch_commits.intersection(&ancestor_commits);
                    let nearest_ancestor_branches = dag.query_heads_ancestors(ancestor_branches)?;
                    Ok(nearest_ancestor_branches)
                };

                let parents = match distance {
                    Distance::AllTheWay {
                        move_by_branches: false,
                    } => {
                        // The `--all` flag for `git prev` isn't useful if all it does
                        // is take you to the root commit for the repository.  Instead,
                        // we assume that the user wanted to get to the root commit for
                        // their current *commit stack*. We filter out commits which
                        // aren't part of the commit stack so that we stop early here.
                        let parents = parent_commits()?;
                        parents.difference(&public_commits)
                    }

                    Distance::AllTheWay {
                        move_by_branches: true,
                    } => {
                        // See above case.
                        let parents = ancestor_branches()?;
                        parents.difference(&public_commits)
                    }

                    Distance::NumCommits {
                        amount: _,
                        move_by_branches: false,
                    } => parent_commits()?,

                    Distance::NumCommits {
                        amount: _,
                        move_by_branches: true,
                    } => ancestor_branches()?,
                };

                sorted_commit_set(repo, dag, &parents)?
            }
        };

        match distance {
            Distance::NumCommits {
                amount,
                move_by_branches: _,
            } => {
                if i == amount {
                    break;
                }
            }

            Distance::AllTheWay {
                move_by_branches: _,
            } => {
                if candidate_commits.is_empty() {
                    break;
                }
            }
        }

        let pluralize = match command {
            Command::Next => Pluralize {
                determiner: None,
                amount: i,
                unit: ("child", "children"),
            },

            Command::Prev => Pluralize {
                determiner: None,
                amount: i,
                unit: ("parent", "parents"),
            },
        };
        let header = format!(
            "Found multiple possible {} commits to go to after traversing {}:",
            pluralize.unit.0, pluralize,
        );

        current_oid = match (towards, candidate_commits.as_slice()) {
            (_, []) => {
                writeln!(
                    effects.get_output_stream(),
                    "{}",
                    glyphs.render(StyledString::styled(
                        format!(
                            "No more {} commits to go to after traversing {}.",
                            pluralize.unit.0, pluralize,
                        ),
                        BaseColor::Yellow.light()
                    ))?
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
                match prompt_select_commit(
                    Some(&header),
                    "",
                    candidate_commits,
                    commit_descriptors,
                )? {
                    Some(oid) => oid,
                    None => {
                        return Ok(None);
                    }
                }
            }
            (None, [_, _, ..]) => {
                writeln!(effects.get_output_stream(), "{header}")?;
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
                        glyphs.render(child.friendly_describe(glyphs)?)?,
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
) -> EyreExitOr<()> {
    let TraverseCommitsOptions {
        num_commits,
        all_the_way,
        move_by_branches,
        oldest,
        newest,
        interactive,
        merge,
        force,
    } = *options;

    let distance = match (all_the_way, num_commits) {
        (false, None) => Distance::NumCommits {
            amount: 1,
            move_by_branches,
        },

        (false, Some(amount)) => Distance::NumCommits {
            amount,
            move_by_branches,
        },

        (true, None) => Distance::AllTheWay { move_by_branches },

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

    let now = SystemTime::now();
    let repo = Repo::from_current_dir()?;
    let head_info = repo.get_head_info()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(
        now,
        match command {
            Command::Next => "next",
            Command::Prev => "prev",
        },
    )?;
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

    let current_oid = advance(
        effects,
        &repo,
        &dag,
        &mut [
            &mut CommitOidDescriptor::new(true)?,
            &mut RelativeTimeDescriptor::new(&repo, SystemTime::now())?,
            &mut BranchesDescriptor::new(
                &repo,
                &head_info,
                &references_snapshot,
                &Redactor::Disabled,
            )?,
            &mut DifferentialRevisionDescriptor::new(&repo, &Redactor::Disabled)?,
            &mut CommitMessageDescriptor::new(&Redactor::Disabled)?,
        ],
        head_oid,
        command,
        distance,
        towards,
    )?;
    let current_oid = match current_oid {
        None => return Ok(Err(ExitCode(1))),
        Some(current_oid) => current_oid,
    };

    let checkout_target: CheckoutTarget = match distance {
        Distance::AllTheWay {
            move_by_branches: false,
        }
        | Distance::NumCommits {
            amount: _,
            move_by_branches: false,
        } => CheckoutTarget::Oid(current_oid),

        Distance::AllTheWay {
            move_by_branches: true,
        }
        | Distance::NumCommits {
            amount: _,
            move_by_branches: true,
        } => {
            let empty = HashSet::new();
            let branches = references_snapshot
                .branch_oid_to_names
                .get(&current_oid)
                .unwrap_or(&empty);

            if branches.is_empty() {
                warn!(?current_oid, "No branches attached to commit with OID");
                CheckoutTarget::Oid(current_oid)
            } else if branches.len() == 1 {
                let branch = branches.iter().next().unwrap();
                CheckoutTarget::Reference(branch.to_owned())
            } else {
                // It's ambiguous which branch the user wants; just check out the commit directly.
                CheckoutTarget::Oid(current_oid)
            }
        }
    };

    let additional_args = {
        let mut args: Vec<OsString> = Vec::new();
        if merge {
            args.push("--merge".into());
        }
        if force {
            args.push("--force".into())
        }
        args
    };
    check_out_commit(
        effects,
        git_run_info,
        &repo,
        &event_log_db,
        event_tx_id,
        Some(checkout_target),
        &CheckOutCommitOptions {
            additional_args,
            ..Default::default()
        },
    )
}

/// Interactively switch to a commit from the smartlog.
pub fn switch(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    switch_options: &SwitchOptions,
) -> EyreExitOr<()> {
    let SwitchOptions {
        interactive,
        branch_name,
        force,
        merge,
        target,
        detach,
    } = switch_options;

    let now = SystemTime::now();
    let repo = Repo::from_current_dir()?;
    let head_info = repo.get_head_info()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "checkout")?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let commits = resolve_default_smartlog_commits(effects, &repo, &mut dag)?;
    let graph = make_smartlog_graph(
        effects,
        &repo,
        &dag,
        &event_replayer,
        event_cursor,
        &commits,
        false,
    )?;

    enum Target {
        /// The (possibly empty) target expression should be used as the initial
        /// query in the commit selector.
        Interactive(String),

        /// The target expression is probably a git revision or reference and
        /// should be passed directly to git for resolution.
        Passthrough(String),

        /// The target expression should be interpreted as a revset.
        Revset(Revset),

        /// No target expression was specified.
        None,
    }
    let initial_query = match (interactive, target) {
        (true, Some(target)) => Target::Interactive(target.to_string()),
        (true, None) => Target::Interactive(String::new()),
        (false, Some(target)) => match repo.revparse_single_commit(target.to_string().as_ref()) {
            Ok(Some(_)) => Target::Passthrough(target.to_string()),
            Ok(None) | Err(_) => Target::Revset(target.clone()),
        },
        (false, None) => Target::None,
    };
    let target: Option<CheckoutTarget> = match initial_query {
        Target::None => None,
        Target::Passthrough(target) => Some(CheckoutTarget::Unknown(target)),
        Target::Interactive(initial_query) => {
            match prompt_select_commit(
                None,
                &initial_query,
                graph.get_commits(),
                &mut [
                    &mut CommitOidDescriptor::new(true)?,
                    &mut RelativeTimeDescriptor::new(&repo, SystemTime::now())?,
                    &mut BranchesDescriptor::new(
                        &repo,
                        &head_info,
                        &references_snapshot,
                        &Redactor::Disabled,
                    )?,
                    &mut DifferentialRevisionDescriptor::new(&repo, &Redactor::Disabled)?,
                    &mut CommitMessageDescriptor::new(&Redactor::Disabled)?,
                ],
            )? {
                Some(oid) => Some(CheckoutTarget::Oid(oid)),
                None => return Ok(Err(ExitCode(1))),
            }
        }
        Target::Revset(target) => {
            let commit_sets = resolve_commits(
                effects,
                &repo,
                &mut dag,
                std::slice::from_ref(&target),
                &ResolveRevsetOptions::default(),
            )?;

            let commit_set = union_all(&commit_sets);
            let commit_set = dag.query_heads(commit_set)?;
            let commits = sorted_commit_set(&repo, &dag, &commit_set)?;

            match commits.as_slice() {
                [commit] => Some(CheckoutTarget::Unknown(commit.get_oid().to_string())),
                [] | [..] => {
                    writeln!(
                        effects.get_error_stream(),
                        "Cannot switch to target: expected '{target}' to contain 1 head, but found {}.",
                        commits.len()
                    )?;
                    writeln!(
                        effects.get_error_stream(),
                        "Target should be a commit or a set of commits with exactly 1 head. Aborting."
                    )?;
                    return Ok(Err(ExitCode(1)));
                }
            }
        }
    };

    let additional_args = {
        let mut args: Vec<OsString> = Vec::new();
        if let Some(branch_name) = branch_name {
            args.push("-b".into());
            args.push(branch_name.into());
        }
        if *force {
            args.push("--force".into());
        }
        if *merge {
            args.push("--merge".into());
        }
        if *detach {
            args.push("--detach".into());
        }
        args
    };

    let exit_code = check_out_commit(
        effects,
        git_run_info,
        &repo,
        &event_log_db,
        event_tx_id,
        target,
        &CheckOutCommitOptions {
            additional_args,
            reset: false,
            render_smartlog: true,
        },
    )?;
    Ok(exit_code)
}
