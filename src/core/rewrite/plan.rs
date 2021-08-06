use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use itertools::Itertools;
use tracing::{instrument, warn};

use crate::core::formatting::printable_styled_string;
use crate::core::graph::{find_path_to_merge_base, CommitGraph, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::git::{NonZeroOid, PatchId, Repo};
use crate::tui::Output;

/// A command that can be applied for either in-memory or on-disk rebases.
#[derive(Debug)]
pub enum RebaseCommand {
    /// Create a label (a reference stored in `refs/rewritten/`) pointing to the
    /// current rebase head for later use.
    CreateLabel { label_name: String },

    /// Move the rebase head to the provided label.
    ResetToLabel { label_name: String },

    /// Move the rebase head to the provided commit.
    ResetToOid { commit_oid: NonZeroOid },

    /// Apply the provided commit on top of the rebase head, and update the
    /// rebase head to point to the newly-applied commit.
    Pick { commit_oid: NonZeroOid },

    /// On-disk rebases only. Register that we want to run cleanup at the end of
    /// the rebase, during the `post-rewrite` hook.
    RegisterExtraPostRewriteHook,

    /// Determine if the current commit is empty. If so, reset the rebase head
    /// to its parent and record that it was empty in the `rewritten-list`.
    DetectEmptyCommit { commit_oid: NonZeroOid },

    /// The commit that would have been applied to the rebase head was already
    /// applied upstream. Skip it and record it in the `rewritten-list`.
    SkipUpstreamAppliedCommit { commit_oid: NonZeroOid },
}

/// Represents a sequence of commands that can be executed to carry out a rebase
/// operation.
#[derive(Debug)]
pub struct RebasePlan {
    pub(super) first_dest_oid: NonZeroOid,
    pub(super) commands: Vec<RebaseCommand>,
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
            RebaseCommand::DetectEmptyCommit { commit_oid } => {
                format!(
                    "exec git branchless hook-detect-empty-commit {}",
                    commit_oid
                )
            }
            RebaseCommand::SkipUpstreamAppliedCommit { commit_oid } => {
                format!(
                    "exec git branchless hook-skip-upstream-applied-commit {}",
                    commit_oid
                )
            }
        }
    }
}

/// Builder for a rebase plan. Unlike regular Git rebases, a `git-branchless`
/// rebase plan can move multiple unrelated subtrees to unrelated destinations.
#[derive(Debug)]
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
#[derive(Debug)]
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
    pub fn describe(&self, output: &mut Output, repo: &Repo) -> eyre::Result<()> {
        match self {
            BuildRebasePlanError::ConstraintCycle { cycle_oids } => {
                writeln!(
                    output,
                    "This operation failed because it would introduce a cycle:"
                )?;

                let glyphs = output.get_glyphs();
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
                        output,
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
        upstream_patch_ids: &HashSet<PatchId>,
        mut acc: Vec<RebaseCommand>,
    ) -> eyre::Result<Vec<RebaseCommand>> {
        let acc = {
            let current_patch_id = match self.repo.find_commit(current_oid)? {
                None => {
                    warn!(?current_oid, "Could not find commit");
                    None
                }
                Some(commit) => self.repo.get_patch_id(&commit)?,
            };
            let patch_already_applied_upstream = match current_patch_id {
                Some(current_patch_id) => upstream_patch_ids.contains(&current_patch_id),
                None => false,
            };
            if patch_already_applied_upstream {
                acc.push(RebaseCommand::SkipUpstreamAppliedCommit {
                    commit_oid: current_oid,
                });
            } else {
                acc.push(RebaseCommand::Pick {
                    commit_oid: current_oid,
                });
                acc.push(RebaseCommand::DetectEmptyCommit {
                    commit_oid: current_oid,
                });
            }
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
                let acc = self.make_rebase_plan_for_current_commit(
                    *only_child_oid,
                    upstream_patch_ids,
                    acc,
                )?;
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
                    acc = self.make_rebase_plan_for_current_commit(
                        *child_oid,
                        upstream_patch_ids,
                        acc,
                    )?;
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
    ) -> eyre::Result<()> {
        self.constraints
            .entry(dest_oid)
            .or_default()
            .insert(source_oid);
        Ok(())
    }

    #[instrument]
    fn collect_descendants(
        &self,
        acc: &mut Vec<Constraint>,
        current_oid: NonZeroOid,
    ) -> eyre::Result<()> {
        // FIXME: O(n^2) algorithm.
        for (child_oid, node) in self.graph.iter() {
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
    fn add_descendant_constraints(&mut self) -> eyre::Result<()> {
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
        output: &mut Output,
        options: &BuildRebasePlanOptions,
    ) -> eyre::Result<Result<Option<RebasePlan>, BuildRebasePlanError>> {
        if options.dump_rebase_constraints {
            writeln!(
                output,
                "Rebase constraints before adding descendants: {:#?}",
                self.get_constraints_sorted_for_debug()
            )?;
        }
        self.add_descendant_constraints()?;
        if options.dump_rebase_constraints {
            writeln!(
                output,
                "Rebase constraints after adding descendants: {:#?}",
                self.get_constraints_sorted_for_debug(),
            )?;
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

            let upstream_patch_ids = self.get_upstream_patch_ids(child_oid, parent_oid)?;
            acc = self.make_rebase_plan_for_current_commit(child_oid, &upstream_patch_ids, acc)?;
        }

        let rebase_plan = first_dest_oid.map(|first_dest_oid| RebasePlan {
            first_dest_oid,
            commands: acc,
        });
        if options.dump_rebase_plan {
            writeln!(output, "Rebase plan: {:#?}", rebase_plan)?;
        }
        Ok(Ok(rebase_plan))
    }

    #[instrument]
    fn get_upstream_patch_ids(
        &self,
        current_oid: NonZeroOid,
        dest_oid: NonZeroOid,
    ) -> eyre::Result<HashSet<PatchId>> {
        let merge_base_oid =
            self.merge_base_db
                .get_merge_base_oid(self.repo, dest_oid, current_oid)?;
        let merge_base_oid = match merge_base_oid {
            None => return Ok(HashSet::new()),
            Some(merge_base_oid) => merge_base_oid,
        };

        let path =
            find_path_to_merge_base(self.repo, self.merge_base_db, dest_oid, merge_base_oid)?;
        let path = match path {
            None => return Ok(HashSet::new()),
            Some(path) => path,
        };

        // FIXME: we may recalculate common patch IDs many times, should be
        // cached.
        let mut result = HashSet::new();
        for commit in path {
            if let Some(patch_id) = self.repo.get_patch_id(&commit)? {
                result.insert(patch_id);
            }
        }
        Ok(result)
    }
}
