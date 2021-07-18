//! Utilities to deal with rewritten commits. See `Event::RewriteEvent` for
//! specifics on commit rewriting.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::time::SystemTime;

use anyhow::Context;
use fn_error_context::context;
use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use os_str_bytes::OsStrBytes;

use crate::commands::gc::mark_commit_reachable;
use crate::core::formatting::printable_styled_string;
use crate::git::{GitRunInfo, MaybeZeroOid, NonZeroOid, Repo};

use super::eventlog::{Event, EventCursor, EventReplayer, EventTransactionId};
use super::formatting::Glyphs;
use super::graph::{find_path_to_merge_base, CommitGraph, MainBranchOid};
use super::mergebase::MergeBaseDb;

/// For a rewritten commit, find the newest version of the commit.
///
/// For example, if we amend commit `abc` into commit `def1`, and then amend
/// `def1` into `def2`, then we can traverse the event log to find out that `def2`
/// is the newest version of `abc`.
///
/// If a commit was rewritten into itself through some chain of events, then
/// returns `None`, rather than the same commit OID.
pub fn find_rewrite_target(
    graph: &CommitGraph,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    oid: NonZeroOid,
) -> Option<MaybeZeroOid> {
    let event = event_replayer.get_cursor_commit_latest_event(event_cursor, oid);
    let event = match event {
        Some(event) => event,
        None => return None,
    };
    match event {
        Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid: MaybeZeroOid::NonZero(old_commit_oid),
            new_commit_oid,
        } => {
            if *old_commit_oid == oid && *new_commit_oid != MaybeZeroOid::NonZero(oid) {
                match new_commit_oid {
                    MaybeZeroOid::Zero => Some(MaybeZeroOid::Zero),
                    MaybeZeroOid::NonZero(new_commit_oid) => {
                        let possible_newer_oid = find_rewrite_target(
                            graph,
                            event_replayer,
                            event_cursor,
                            *new_commit_oid,
                        );
                        match possible_newer_oid {
                            Some(newer_commit_oid) => Some(newer_commit_oid),
                            None => Some(MaybeZeroOid::NonZero(*new_commit_oid)),
                        }
                    }
                }
            } else {
                None
            }
        }

        Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid: MaybeZeroOid::Zero,
            new_commit_oid: _,
        }
        | Event::RefUpdateEvent { .. }
        | Event::CommitEvent { .. }
        | Event::HideEvent { .. }
        | Event::UnhideEvent { .. } => None,
    }
}

/// Find commits which have been "abandoned" in the commit graph.
///
/// A commit is considered "abandoned" if it is visible, but one of its parents
/// is hidden.
pub fn find_abandoned_children(
    graph: &CommitGraph,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    oid: NonZeroOid,
) -> Option<(NonZeroOid, Vec<NonZeroOid>)> {
    let rewritten_oid = match find_rewrite_target(graph, event_replayer, event_cursor, oid)? {
        MaybeZeroOid::NonZero(rewritten_oid) => rewritten_oid,
        MaybeZeroOid::Zero => oid,
    };

    // Adjacent main branch commits are not linked in the commit graph, but if
    // the user rewrote a main branch commit, then we may need to restack
    // subsequent main branch commits. Find the real set of children commits so
    // that we can do this.
    let mut real_children_oids = graph[&oid].children.clone();
    let additional_children_oids: HashSet<NonZeroOid> = graph
        .iter()
        .filter_map(|(possible_child_oid, possible_child_node)| {
            if real_children_oids.contains(possible_child_oid) {
                // Don't bother looking up the parents for commits we are
                // already including.
                None
            } else if possible_child_node
                .commit
                .get_parent_oids()
                .into_iter()
                .any(|parent_oid| parent_oid == oid)
            {
                Some(possible_child_oid)
            } else {
                None
            }
        })
        .copied()
        .collect();
    real_children_oids.extend(additional_children_oids);

    let visible_children_oids = real_children_oids
        .iter()
        .filter(|child_oid| graph[child_oid].is_visible)
        .copied()
        .collect();
    Some((rewritten_oid, visible_children_oids))
}

#[derive(Debug)]
enum RebaseCommand {
    CreateLabel { label_name: String },
    ResetToLabel { label_name: String },
    ResetToOid { commit_oid: NonZeroOid },
    Pick { commit_oid: NonZeroOid },
    RegisterExtraPostRewriteHook,
}

/// Represents a sequence of commands that can be executed to carry out a rebase
/// operation.
#[derive(Debug)]
pub struct RebasePlan {
    first_dest_oid: NonZeroOid,
    commands: Vec<RebaseCommand>,
}

impl ToString for RebaseCommand {
    fn to_string(&self) -> String {
        match self {
            RebaseCommand::CreateLabel { label_name } => format!("label {}", label_name),
            RebaseCommand::ResetToLabel { label_name } => format!("reset {}", label_name),
            RebaseCommand::ResetToOid { commit_oid: oid } => format!("reset {}", oid),
            RebaseCommand::Pick { commit_oid } => format!("pick {}", commit_oid),
            RebaseCommand::RegisterExtraPostRewriteHook => {
                "exec git branchless hook-register-extra-post-rewrite-hook".to_string()
            }
        }
    }
}

/// Builder for a rebase plan. Unlike regular Git rebases, a `git-branchless`
/// rebase plan can move multiple unrelated subtrees to unrelated destinations.
pub struct RebasePlanBuilder<'repo> {
    repo: &'repo Repo,
    graph: &'repo CommitGraph<'repo>,
    merge_base_db: &'repo MergeBaseDb<'repo>,
    main_branch_oid: NonZeroOid,

    /// There is a mapping from from `x` to `y` if `x` must be applied before
    /// `y`.
    constraints: HashMap<NonZeroOid, HashSet<NonZeroOid>>,
    used_labels: HashSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Constraint {
    parent_oid: NonZeroOid,
    child_oid: NonZeroOid,
}

/// Options used to build a rebase plan.
pub struct BuildRebasePlanOptions {
    /// Print the rebase constraints for debugging.
    pub dump_rebase_constraints: bool,

    /// Print the rebase plan for debugging.
    pub dump_rebase_plan: bool,
}

/// An error caused when attempting to build a rebase plan.
pub enum BuildRebasePlanError {
    /// There was a cycle in the requested graph to be built.
    ConstraintCycle {
        /// The OIDs of the commits in the cycle. The first and the last OIDs are the same.
        cycle_oids: Vec<NonZeroOid>,
    },
}

impl BuildRebasePlanError {
    /// Write the error message to `out`.
    pub fn describe(
        &self,
        out: &mut impl Write,
        glyphs: &Glyphs,
        repo: &Repo,
    ) -> anyhow::Result<()> {
        match self {
            BuildRebasePlanError::ConstraintCycle { cycle_oids } => {
                writeln!(
                    out,
                    "This operation failed because it would introduce a cycle:"
                )?;

                let num_cycle_commits = cycle_oids.len();
                for (i, oid) in cycle_oids.iter().enumerate() {
                    let (char1, char2, char3) = if i == 0 {
                        (
                            glyphs.cycle_upper_left_corner,
                            glyphs.cycle_horizontal_line,
                            glyphs.cycle_arrow,
                        )
                    } else if i + 1 == num_cycle_commits {
                        (
                            glyphs.cycle_lower_left_corner,
                            glyphs.cycle_horizontal_line,
                            glyphs.cycle_horizontal_line,
                        )
                    } else {
                        (glyphs.cycle_vertical_line, " ", " ")
                    };
                    writeln!(
                        out,
                        "{}{}{} {}",
                        char1,
                        char2,
                        char3,
                        printable_styled_string(
                            &glyphs,
                            repo.friendly_describe_commit_from_oid(*oid)?
                        )?,
                    )?;
                }
            }
        }
        Ok(())
    }
}

impl<'repo> RebasePlanBuilder<'repo> {
    /// Constructor.
    pub fn new(
        repo: &'repo Repo,
        graph: &'repo CommitGraph,
        merge_base_db: &'repo MergeBaseDb,
        main_branch_oid: &MainBranchOid,
    ) -> Self {
        let MainBranchOid(main_branch_oid) = main_branch_oid;
        RebasePlanBuilder {
            repo,
            graph,
            merge_base_db,
            main_branch_oid: *main_branch_oid,
            constraints: Default::default(),
            used_labels: Default::default(),
        }
    }

    fn make_label_name(&mut self, preferred_name: impl Into<String>) -> String {
        let mut preferred_name = preferred_name.into();
        if !self.used_labels.contains(&preferred_name) {
            self.used_labels.insert(preferred_name.clone());
            preferred_name
        } else {
            preferred_name.push('\'');
            self.make_label_name(preferred_name)
        }
    }

    fn make_rebase_plan_for_current_commit(
        &mut self,
        current_oid: NonZeroOid,
        mut acc: Vec<RebaseCommand>,
    ) -> anyhow::Result<Vec<RebaseCommand>> {
        let acc = {
            acc.push(RebaseCommand::Pick {
                commit_oid: current_oid,
            });
            acc
        };
        let child_nodes: Vec<NonZeroOid> = {
            let mut child_nodes: Vec<NonZeroOid> = self
                .constraints
                .entry(current_oid)
                .or_default()
                .iter()
                .copied()
                .collect();
            child_nodes.sort_unstable();
            child_nodes
        };

        match child_nodes.as_slice() {
            [] => Ok(acc),
            [only_child_oid] => {
                let acc = self.make_rebase_plan_for_current_commit(*only_child_oid, acc)?;
                Ok(acc)
            }
            children => {
                let command_num = acc.len();
                let label_name = self.make_label_name(format!("label-{}", command_num));
                let mut acc = acc;
                acc.push(RebaseCommand::CreateLabel {
                    label_name: label_name.clone(),
                });
                for child_oid in children {
                    acc = self.make_rebase_plan_for_current_commit(*child_oid, acc)?;
                    acc.push(RebaseCommand::ResetToLabel {
                        label_name: label_name.clone(),
                    });
                }
                Ok(acc)
            }
        }
    }

    /// Generate a sequence of rebase steps that cause the subtree at `source_oid`
    /// to be rebased on top of `dest_oid`.
    pub fn move_subtree(
        &mut self,
        source_oid: NonZeroOid,
        dest_oid: NonZeroOid,
    ) -> anyhow::Result<()> {
        self.constraints
            .entry(dest_oid)
            .or_default()
            .insert(source_oid);
        Ok(())
    }

    #[context("Collecting descendants of commit: {:?}", current_oid)]
    fn collect_descendants(
        &self,
        acc: &mut Vec<Constraint>,
        current_oid: NonZeroOid,
    ) -> anyhow::Result<()> {
        // FIXME: O(n^2) algorithm.
        for (child_oid, node) in self.graph {
            if node.commit.get_parent_oids().contains(&current_oid) {
                acc.push(Constraint {
                    parent_oid: current_oid,
                    child_oid: *child_oid,
                });
                self.collect_descendants(acc, *child_oid)?;
            }
        }

        // Calculate the commits along the main branch to be moved if this is a
        // constraint for a main branch commit.
        //
        // FIXME: The below logic is not quite correct when it comes to
        // multi-parent commits. It's possible to have a topology where there
        // are multiple paths to a main branch node:
        //
        // ```text
        // O main node
        // |\
        // | o some node
        // | |
        // o some other node
        // | |
        // |/
        // o visible node
        // ```
        //
        // In this case, `find_path_to_merge_base` will only find the shortest
        // path and move those commits. In principle, it should be possible to
        // find all paths to the main node. The difficulty is that one has to be
        // careful not to "overshoot" the main node and traverse history all the
        // way to the initial commit for performance reasons. Example:
        //
        // ```text
        // o initial commit
        // :
        // : one million commits...
        // :
        // o some ancestor node of the main node
        // |\
        // | |
        // O main node
        // | |
        // | o some node
        // | |
        // o some other node
        // | |
        // |/
        // o visible node
        // ```
        //
        // The above case can be handled by calculating all the merge-bases with
        // the main branch whenever we find a multi-parent commit. The below
        // case is even trickier:
        //
        //
        // ```text
        // o initial commit
        // |\
        // : :
        // : : one million commits...
        // : :
        // | |
        // O main node
        // | |
        // | o some node
        // | |
        // o some other node
        // | |
        // |/
        // o visible node
        // ```
        //
        // Even determining the merge-bases with main of the parent nodes could
        // take ages to complete. This could potentially be either by limiting
        // the traversal to a certain amount and giving up, or leveraging `git
        // commit-graph`: https://git-scm.com/docs/commit-graph. (I believe that
        // `libgit2` does not currently have support for commit graphs.)
        let is_main = match self.graph.get(&current_oid) {
            Some(node) => node.is_main,
            None => true,
        };
        if is_main {
            // This must be a main branch commit. We need to collect its
            // descendants, which don't appear in the commit graph.
            let path = find_path_to_merge_base(
                self.repo,
                self.merge_base_db,
                self.main_branch_oid,
                current_oid,
            )?;
            if let Some(path) = path {
                let mut parent_oid = current_oid;
                for child_commit in path
                    .into_iter()
                    // Start from the node and traverse children towards the main branch.
                    .rev()
                    // Skip the starting node itself, as it already has a constraint.
                    .skip(1)
                {
                    let child_oid = child_commit.get_oid();
                    acc.push(Constraint {
                        parent_oid,
                        child_oid,
                    });
                    // We've hit a node that is in the graph, so further
                    // constraints should be added by the above code path.
                    if self.graph.contains_key(&child_oid) {
                        break;
                    }
                    parent_oid = child_oid;
                }
            }
        }
        Ok(())
    }

    /// Add additional edges to the constraint graph for each descendant commit
    /// of a referred-to commit. This adds enough information to the constraint
    /// graph that it now represents the actual end-state commit graph that we
    /// want to create, not just a list of constraints.
    fn add_descendant_constraints(&mut self) -> anyhow::Result<()> {
        let all_descendants_of_constrained_nodes = {
            let mut acc = Vec::new();
            for parent_oid in self.constraints.values().flatten().cloned() {
                self.collect_descendants(&mut acc, parent_oid)?;
            }
            acc
        };
        for Constraint {
            parent_oid,
            child_oid,
        } in all_descendants_of_constrained_nodes
        {
            self.constraints
                .entry(parent_oid)
                .or_default()
                .insert(child_oid);
        }
        Ok(())
    }

    fn check_for_cycles_helper(
        &self,
        path: &mut Vec<NonZeroOid>,
        current_oid: NonZeroOid,
    ) -> Result<(), BuildRebasePlanError> {
        if path.contains(&current_oid) {
            path.push(current_oid);
            return Err(BuildRebasePlanError::ConstraintCycle {
                cycle_oids: path.clone(),
            });
        }

        path.push(current_oid);
        if let Some(child_oids) = self.constraints.get(&current_oid) {
            for child_oid in child_oids.iter().sorted() {
                self.check_for_cycles_helper(path, *child_oid)?;
            }
        }
        Ok(())
    }

    fn check_for_cycles(&self) -> Result<(), BuildRebasePlanError> {
        // FIXME: O(n^2) algorithm.
        for oid in self.constraints.keys().sorted() {
            self.check_for_cycles_helper(&mut Vec::new(), *oid)?;
        }
        Ok(())
    }

    fn find_roots(&self) -> Vec<Constraint> {
        let unconstrained_nodes = {
            let mut unconstrained_nodes: HashSet<NonZeroOid> =
                self.constraints.keys().copied().collect();
            for child_oid in self.constraints.values().flatten().copied() {
                unconstrained_nodes.remove(&child_oid);
            }
            unconstrained_nodes
        };
        let mut root_edges: Vec<Constraint> = unconstrained_nodes
            .into_iter()
            .flat_map(|unconstrained_oid| {
                self.constraints[&unconstrained_oid]
                    .iter()
                    .copied()
                    .map(move |child_oid| Constraint {
                        parent_oid: unconstrained_oid,
                        child_oid,
                    })
            })
            .collect();
        root_edges.sort_unstable();
        root_edges
    }

    fn get_constraints_sorted_for_debug(&self) -> Vec<(&NonZeroOid, Vec<&NonZeroOid>)> {
        self.constraints
            .iter()
            .map(|(k, v)| (k, v.iter().sorted().collect::<Vec<_>>()))
            .sorted()
            .collect::<Vec<_>>()
    }

    /// Create the rebase plan. Returns `None` if there were no commands in the rebase plan.
    pub fn build(
        mut self,
        options: &BuildRebasePlanOptions,
    ) -> anyhow::Result<Result<Option<RebasePlan>, BuildRebasePlanError>> {
        if options.dump_rebase_constraints {
            println!(
                "Rebase constraints before adding descendants: {:#?}",
                self.get_constraints_sorted_for_debug()
            );
        }
        self.add_descendant_constraints()?;
        if options.dump_rebase_constraints {
            println!(
                "Rebase constraints after adding descendants: {:#?}",
                self.get_constraints_sorted_for_debug(),
            );
        }

        if let Err(err) = self.check_for_cycles() {
            return Ok(Err(err));
        }

        let roots = self.find_roots();
        let mut acc = vec![RebaseCommand::RegisterExtraPostRewriteHook];
        let mut first_dest_oid = None;
        for Constraint {
            parent_oid,
            child_oid,
        } in roots
        {
            first_dest_oid.get_or_insert(parent_oid);
            acc.push(RebaseCommand::ResetToOid {
                commit_oid: parent_oid,
            });
            acc = self.make_rebase_plan_for_current_commit(child_oid, acc)?;
        }

        let rebase_plan = first_dest_oid.map(|first_dest_oid| RebasePlan {
            first_dest_oid,
            commands: acc,
        });
        if options.dump_rebase_plan {
            println!("Rebase plan: {:#?}", rebase_plan);
        }
        Ok(Ok(rebase_plan))
    }
}

enum RebaseInMemoryResult {
    Succeeded {
        rewritten_oids: Vec<(NonZeroOid, MaybeZeroOid)>,
    },
    CannotRebaseMergeCommit {
        commit_oid: NonZeroOid,
    },
    MergeConflict {
        commit_oid: NonZeroOid,
    },
}

#[context("Rebasing in memory")]
fn rebase_in_memory(
    glyphs: &Glyphs,
    repo: &Repo,
    rebase_plan: &RebasePlan,
    options: &ExecuteRebasePlanOptions,
) -> anyhow::Result<RebaseInMemoryResult> {
    let ExecuteRebasePlanOptions {
        now,
        // Transaction ID will be passed to the `post-rewrite` hook via
        // environment variable.
        event_tx_id: _,
        preserve_timestamps,
        force_in_memory: _,
        force_on_disk: _,
    } = options;

    let mut current_oid = rebase_plan.first_dest_oid;
    let mut labels: HashMap<String, NonZeroOid> = HashMap::new();
    let mut rewritten_oids: Vec<(NonZeroOid, MaybeZeroOid)> = Vec::new();

    let mut i = 0;
    let num_picks = rebase_plan
        .commands
        .iter()
        .filter(|command| match command {
            RebaseCommand::CreateLabel { .. }
            | RebaseCommand::ResetToLabel { .. }
            | RebaseCommand::ResetToOid { .. }
            | RebaseCommand::RegisterExtraPostRewriteHook => false,
            RebaseCommand::Pick { .. } => true,
        })
        .count();

    for command in rebase_plan.commands.iter() {
        match command {
            RebaseCommand::CreateLabel { label_name } => {
                labels.insert(label_name.clone(), current_oid);
            }
            RebaseCommand::ResetToLabel { label_name } => {
                current_oid = match labels.get(label_name) {
                    Some(oid) => *oid,
                    None => anyhow::bail!("BUG: no associated OID for label: {}", label_name),
                };
            }
            RebaseCommand::ResetToOid { commit_oid } => {
                current_oid = *commit_oid;
            }
            RebaseCommand::Pick { commit_oid } => {
                let current_commit = repo
                    .find_commit(current_oid)
                    .with_context(|| format!("Finding current commit by OID: {:?}", current_oid))?;
                let current_commit = match current_commit {
                    Some(commit) => commit,
                    None => {
                        anyhow::bail!("Unable to find current commit with OID: {:?}", current_oid)
                    }
                };
                let commit_to_apply = repo
                    .find_commit(*commit_oid)
                    .with_context(|| format!("Finding commit to apply by OID: {:?}", commit_oid))?;
                let commit_to_apply = match commit_to_apply {
                    Some(commit) => commit,
                    None => {
                        anyhow::bail!("Unable to find commit to apply with OID: {:?}", current_oid)
                    }
                };
                i += 1;

                let commit_description =
                    printable_styled_string(glyphs, commit_to_apply.friendly_describe()?)?;
                let template = format!("[{}/{}] {{spinner}} {{wide_msg}}", i, num_picks);
                let progress = ProgressBar::new_spinner();
                progress.set_style(ProgressStyle::default_spinner().template(&template.trim()));
                progress.set_message("Starting");
                progress.enable_steady_tick(100);

                if commit_to_apply.get_parent_count() > 1 {
                    return Ok(RebaseInMemoryResult::CannotRebaseMergeCommit {
                        commit_oid: *commit_oid,
                    });
                };

                progress.set_message(format!("Applying patch for commit: {}", commit_description));
                let mut rebased_index =
                    repo.cherrypick_commit(&commit_to_apply, &current_commit, 0, None)?;

                progress.set_message(format!(
                    "Checking for merge conflicts: {}",
                    commit_description
                ));
                if rebased_index.has_conflicts() {
                    return Ok(RebaseInMemoryResult::MergeConflict {
                        commit_oid: *commit_oid,
                    });
                }

                progress.set_message(format!(
                    "Writing commit data to disk: {}",
                    commit_description
                ));
                let commit_tree_oid = repo
                    .write_index_to_tree(&mut rebased_index)
                    .with_context(|| "Converting index to tree")?;
                let commit_tree = match repo.find_tree(commit_tree_oid)? {
                    Some(tree) => tree,
                    None => anyhow::bail!(
                        "Could not find freshly-written tree for OID: {:?}",
                        commit_tree_oid
                    ),
                };
                let commit_message = commit_to_apply.get_message_raw()?;
                let commit_message = match commit_message.to_str() {
                    Some(message) => message,
                    None => anyhow::bail!(
                        "Could not decode commit message for commit: {:?}",
                        commit_oid
                    ),
                };

                progress.set_message(format!("Committing to repository: {}", commit_description));
                let committer_signature = if *preserve_timestamps {
                    commit_to_apply.get_committer()
                } else {
                    commit_to_apply.get_committer().update_timestamp(*now)?
                };
                let rebased_commit_oid = repo
                    .create_commit(
                        None,
                        &commit_to_apply.get_author(),
                        &committer_signature,
                        commit_message,
                        &commit_tree,
                        &[&current_commit],
                    )
                    .with_context(|| "Applying rebased commit")?;
                rewritten_oids.push((*commit_oid, MaybeZeroOid::NonZero(rebased_commit_oid)));
                current_oid = rebased_commit_oid;

                let commit_description = printable_styled_string(
                    glyphs,
                    repo.friendly_describe_commit_from_oid(rebased_commit_oid)?,
                )?;
                progress.finish_with_message(format!("Committed as: {}", commit_description));
            }
            RebaseCommand::RegisterExtraPostRewriteHook => {
                // Do nothing. We'll carry out post-rebase operations after the
                // in-memory rebase completes.
            }
        }
    }

    Ok(RebaseInMemoryResult::Succeeded { rewritten_oids })
}

/// Given a list of rewritten OIDs, move the branches attached to those OIDs
/// from their old commits to their new commits. Invoke the
/// `reference-transaction` hook when done.
pub fn move_branches<'a>(
    git_run_info: &GitRunInfo,
    repo: &'a Repo,
    event_tx_id: EventTransactionId,
    rewritten_oids_map: &'a HashMap<NonZeroOid, MaybeZeroOid>,
) -> anyhow::Result<()> {
    let branch_oid_to_names = repo.get_branch_oid_to_names()?;

    // We may experience an error in the case of a branch move. Ideally, we
    // would use `git2::Transaction::commit`, which stops the transaction at the
    // first error, but we don't know which references we successfully committed
    // in that case. Instead, we just do things non-atomically and record which
    // ones succeeded. See https://github.com/libgit2/libgit2/issues/5918
    let mut branch_moves: Vec<(NonZeroOid, NonZeroOid, &OsStr)> = Vec::new();
    let mut branch_move_err: Option<anyhow::Error> = None;
    'outer: for (old_oid, names) in branch_oid_to_names.iter() {
        let new_oid = match rewritten_oids_map.get(&old_oid) {
            Some(new_oid) => new_oid,
            None => continue,
        };
        let new_oid = match new_oid {
            MaybeZeroOid::NonZero(new_oid) => new_oid,
            MaybeZeroOid::Zero => todo!("handle branch deletions"),
        };
        let new_commit = match repo.find_commit(*new_oid) {
            Ok(Some(commit)) => commit,
            Ok(None) => {
                branch_move_err = Some(anyhow::anyhow!(
                    "Could not find newly-rewritten commit with old OID: {:?}, new OID: {:?}",
                    old_oid,
                    new_oid
                ));
                break 'outer;
            }
            Err(err) => {
                branch_move_err = Some(err);
                break 'outer;
            }
        };

        let mut names: Vec<_> = names.iter().collect();
        // Sort for determinism in tests.
        names.sort_unstable();
        for name in names {
            if let Err(err) =
                repo.create_reference(name, new_commit.get_oid(), true, "move branches")
            {
                branch_move_err = Some(err);
                break 'outer;
            }
            branch_moves.push((*old_oid, *new_oid, name))
        }
    }

    let branch_moves_stdin: Vec<u8> = branch_moves
        .into_iter()
        .flat_map(|(old_oid, new_oid, name)| {
            let mut line = Vec::new();
            line.extend(old_oid.to_string().as_bytes());
            line.push(b' ');
            line.extend(new_oid.to_string().as_bytes());
            line.push(b' ');
            line.extend(name.to_raw_bytes().iter());
            line.push(b'\n');
            line
        })
        .collect();
    let branch_moves_stdin = OsStrBytes::from_raw_bytes(branch_moves_stdin)
        .with_context(|| "Encoding branch moves stdin")?;
    let branch_moves_stdin = OsString::from(branch_moves_stdin);
    git_run_info.run_hook(
        repo,
        "reference-transaction",
        event_tx_id,
        &["committed"],
        Some(branch_moves_stdin),
    )?;
    match branch_move_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn post_rebase_in_memory(
    git_run_info: &GitRunInfo,
    repo: &Repo,
    rewritten_oids: &[(NonZeroOid, MaybeZeroOid)],
    options: &ExecuteRebasePlanOptions,
) -> anyhow::Result<isize> {
    let ExecuteRebasePlanOptions {
        now: _,
        event_tx_id,
        preserve_timestamps: _,
        force_in_memory: _,
        force_on_disk: _,
    } = options;

    // Note that if an OID has been mapped to multiple other OIDs, then the last
    // mapping wins. (This corresponds to the last applied rebase operation.)
    let rewritten_oids_map: HashMap<NonZeroOid, MaybeZeroOid> =
        rewritten_oids.iter().copied().collect();

    for new_oid in rewritten_oids_map.values() {
        if let MaybeZeroOid::NonZero(new_oid) = new_oid {
            mark_commit_reachable(repo, *new_oid)?;
        }
    }

    let head_info = repo.get_head_info()?;
    if head_info.oid.is_some() {
        // Avoid moving the branch which HEAD points to, or else the index will show
        // a lot of changes in the working copy.
        head_info.detach_head()?;
    }

    move_branches(git_run_info, repo, *event_tx_id, &rewritten_oids_map)?;

    // Call the `post-rewrite` hook only after moving branches so that we don't
    // produce a spurious abandoned-branch warning.
    let post_rewrite_stdin: String = rewritten_oids
        .iter()
        .map(|(old_oid, new_oid)| format!("{} {}\n", old_oid.to_string(), new_oid.to_string()))
        .collect();
    let post_rewrite_stdin = OsString::from(post_rewrite_stdin);
    git_run_info.run_hook(
        repo,
        "post-rewrite",
        *event_tx_id,
        &["rebase"],
        Some(post_rewrite_stdin),
    )?;

    if let Some(head_oid) = head_info.oid {
        if let Some(new_head_oid) = rewritten_oids_map.get(&head_oid) {
            let head_target = match head_info.get_branch_name() {
                Some(head_branch) => head_branch.to_string(),
                None => new_head_oid.to_string(),
            };
            let result = git_run_info.run(Some(*event_tx_id), &["checkout", &head_target])?;
            if result != 0 {
                return Ok(result);
            }
        }
    }

    Ok(0)
}

/// Rebase on-disk. We don't use `git2`'s `Rebase` machinery because it ends up
/// being too slow.
#[context("Rebasing on disk")]
fn rebase_on_disk(
    git_run_info: &GitRunInfo,
    repo: &Repo,
    rebase_plan: &RebasePlan,
    options: &ExecuteRebasePlanOptions,
) -> anyhow::Result<isize> {
    let ExecuteRebasePlanOptions {
        // `git rebase` will make its own timestamp.
        now: _,
        event_tx_id,
        preserve_timestamps,
        force_in_memory: _,
        force_on_disk: _,
    } = options;

    let progress = ProgressBar::new_spinner();
    progress.enable_steady_tick(100);
    progress.set_message("Initializing rebase");

    let head_info = repo.get_head_info()?;

    let current_operation_type = repo.get_current_operation_type();
    if let Some(current_operation_type) = current_operation_type {
        progress.finish_and_clear();
        println!(
            "A {} operation is already in progress.",
            current_operation_type
        );
        println!(
            "Run git {0} --continue or git {0} --abort to resolve it and proceed.",
            current_operation_type
        );
        return Ok(1);
    }

    let rebase_state_dir = repo.get_rebase_state_dir_path();
    std::fs::create_dir_all(&rebase_state_dir).with_context(|| {
        format!(
            "Creating rebase state directory at: {:?}",
            &rebase_state_dir
        )
    })?;

    // Mark this rebase as an interactive rebase. For whatever reason, if this
    // is not marked as an interactive rebase, then some rebase plans fail with
    // this error:
    //
    // ```
    // BUG: builtin/rebase.c:1178: Unhandled rebase type 1
    // ```
    let interactive_file_path = rebase_state_dir.join("interactive");
    std::fs::write(&interactive_file_path, "")
        .with_context(|| format!("Writing interactive to: {:?}", &interactive_file_path))?;

    if head_info.oid.is_some() {
        let repo_head_file_path = repo.get_path().join("HEAD");
        let orig_head_file_path = repo.get_path().join("ORIG_HEAD");
        std::fs::copy(&repo_head_file_path, &orig_head_file_path)
            .with_context(|| format!("Copying `HEAD` to: {:?}", &orig_head_file_path))?;
        // `head-name` appears to be purely for UX concerns. Git will warn if the
        // file isn't found.
        let head_name_file_path = rebase_state_dir.join("head-name");
        std::fs::write(
            &head_name_file_path,
            head_info.get_branch_name().unwrap_or("detached HEAD"),
        )
        .with_context(|| format!("Writing head-name to: {:?}", &head_name_file_path))?;

        // Dummy `head` file. We will `reset` to the appropriate commit as soon as
        // we start the rebase.
        let rebase_merge_head_file_path = rebase_state_dir.join("head");
        std::fs::write(
            &rebase_merge_head_file_path,
            rebase_plan.first_dest_oid.to_string(),
        )
        .with_context(|| format!("Writing head to: {:?}", &rebase_merge_head_file_path))?;
    }

    // Dummy `onto` file. We may be rebasing onto a set of unrelated
    // nodes in the same operation, so there may not be a single "onto" node to
    // refer to.
    let onto_file_path = rebase_state_dir.join("onto");
    std::fs::write(&onto_file_path, rebase_plan.first_dest_oid.to_string()).with_context(|| {
        format!(
            "Writing onto {:?} to: {:?}",
            &rebase_plan.first_dest_oid, &onto_file_path
        )
    })?;

    let todo_file_path = rebase_state_dir.join("git-rebase-todo");
    std::fs::write(
        &todo_file_path,
        rebase_plan
            .commands
            .iter()
            .map(|command| format!("{}\n", command.to_string()))
            .collect::<String>(),
    )
    .with_context(|| {
        format!(
            "Writing `git-rebase-todo` to: {:?}",
            todo_file_path.as_path()
        )
    })?;

    let end_file_path = rebase_state_dir.join("end");
    std::fs::write(
        end_file_path.as_path(),
        format!("{}\n", rebase_plan.commands.len()),
    )
    .with_context(|| format!("Writing `end` to: {:?}", end_file_path.as_path()))?;

    if *preserve_timestamps {
        let cdate_is_adate_file_path = rebase_state_dir.join("cdate_is_adate");
        std::fs::write(&cdate_is_adate_file_path, "")
            .with_context(|| "Writing `cdate_is_adate` option file")?;
    }

    progress.finish_and_clear();
    println!("Calling Git for on-disk rebase...");
    let result = git_run_info.run(Some(*event_tx_id), &["rebase", "--continue"])?;
    Ok(result)
}

/// Options to use when executing a `RebasePlan`.
#[derive(Clone, Debug)]
pub struct ExecuteRebasePlanOptions {
    /// The time which should be recorded for this event.
    pub now: SystemTime,

    /// The transaction ID for this event.
    pub event_tx_id: EventTransactionId,

    /// If `true`, any rewritten commits will keep the same authored and
    /// committed timestamps. If `false`, the committed timestamps will be updated
    /// to the current time.
    pub preserve_timestamps: bool,

    /// Force an in-memory rebase (as opposed to an on-disk rebase).
    pub force_in_memory: bool,

    /// Force an on-disk rebase (as opposed to an in-memory rebase).
    pub force_on_disk: bool,
}

/// Execute the provided rebase plan. Returns the exit status (zero indicates
/// success).
pub fn execute_rebase_plan(
    glyphs: &Glyphs,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    rebase_plan: &RebasePlan,
    options: &ExecuteRebasePlanOptions,
) -> anyhow::Result<isize> {
    let ExecuteRebasePlanOptions {
        now: _,
        event_tx_id: _,
        preserve_timestamps: _,
        force_in_memory,
        force_on_disk,
    } = options;

    if !force_on_disk {
        println!("Attempting rebase in-memory...");
        match rebase_in_memory(glyphs, &repo, &rebase_plan, &options)? {
            RebaseInMemoryResult::Succeeded { rewritten_oids } => {
                post_rebase_in_memory(git_run_info, repo, &rewritten_oids, &options)?;
                println!("In-memory rebase succeeded.");
                return Ok(0);
            }
            RebaseInMemoryResult::CannotRebaseMergeCommit { commit_oid } => {
                println!(
                    "Merge commits currently can't be rebased with `git move`. The merge commit was: {}",
                    printable_styled_string(glyphs, repo.friendly_describe_commit_from_oid(commit_oid)?)?,
                );
                return Ok(1);
            }
            RebaseInMemoryResult::MergeConflict { commit_oid } => {
                if *force_in_memory {
                    println!(
                        "Merge conflict. The conflicting commit was: {}",
                        printable_styled_string(
                            glyphs,
                            repo.friendly_describe_commit_from_oid(commit_oid)?,
                        )?,
                    );
                    println!("Aborting since an in-memory rebase was requested.");
                    return Ok(1);
                } else {
                    println!(
                        "Merge conflict, falling back to rebase on-disk. The conflicting commit was: {}",
                        printable_styled_string(glyphs, repo.friendly_describe_commit_from_oid(commit_oid)?)?,
                    );
                }
            }
        }
    }

    if !force_in_memory {
        let result = rebase_on_disk(git_run_info, repo, &rebase_plan, &options)?;
        return Ok(result);
    }

    anyhow::bail!(
        "Both force_in_memory and force_on_disk were requested, but these options conflict"
    )
}

#[cfg(test)]
mod tests {
    use crate::core::eventlog::EventLogDb;
    use crate::core::graph::{make_graph, BranchOids, HeadOid, MainBranchOid};
    use crate::core::mergebase::MergeBaseDb;
    use crate::testing::{make_git, Git, GitRunOptions};

    use super::*;

    fn find_rewrite_target_helper(
        git: &Git,
        oid: NonZeroOid,
    ) -> anyhow::Result<Option<MaybeZeroOid>> {
        let repo = git.get_repo()?;
        let conn = repo.get_db_conn()?;
        let merge_base_db = MergeBaseDb::new(&conn)?;
        let event_log_db = EventLogDb::new(&conn)?;
        let event_replayer = EventReplayer::from_event_log_db(&repo, &event_log_db)?;
        let event_cursor = event_replayer.make_default_cursor();
        let head_oid = repo.get_head_info()?.oid;
        let main_branch_oid = repo.get_main_branch_oid()?;
        let branch_oid_to_names = repo.get_branch_oid_to_names()?;
        let graph = make_graph(
            &repo,
            &merge_base_db,
            &event_replayer,
            event_cursor,
            &HeadOid(head_oid),
            &MainBranchOid(main_branch_oid),
            &BranchOids(branch_oid_to_names.keys().copied().collect()),
            true,
        )?;

        let rewrite_target = find_rewrite_target(&graph, &event_replayer, event_cursor, oid);
        Ok(rewrite_target)
    }

    #[test]
    fn test_find_rewrite_target() -> anyhow::Result<()> {
        let git = make_git()?;

        git.init_repo()?;
        let commit_time = 1;
        let old_oid = git.commit_file("test1", commit_time)?;

        {
            git.run(&["commit", "--amend", "-m", "test1 amended once"])?;
            let new_oid: MaybeZeroOid = {
                let (stdout, _stderr) = git.run(&["rev-parse", "HEAD"])?;
                stdout.trim().parse()?
            };
            let rewrite_target = find_rewrite_target_helper(&git, old_oid)?;
            assert_eq!(rewrite_target, Some(new_oid));
        }

        {
            git.run(&["commit", "--amend", "-m", "test1 amended twice"])?;
            let new_oid: MaybeZeroOid = {
                let (stdout, _stderr) = git.run(&["rev-parse", "HEAD"])?;
                stdout.trim().parse()?
            };
            let rewrite_target = find_rewrite_target_helper(&git, old_oid)?;
            assert_eq!(rewrite_target, Some(new_oid));
        }

        {
            git.run_with_options(
                &["commit", "--amend", "-m", "create test1.txt"],
                &GitRunOptions {
                    time: commit_time,
                    ..Default::default()
                },
            )?;
            let rewrite_target = find_rewrite_target_helper(&git, old_oid)?;
            assert_eq!(rewrite_target, None);
        }

        Ok(())
    }
}
