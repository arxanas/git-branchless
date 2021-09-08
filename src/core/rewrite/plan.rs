use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::ops::Sub;
use std::path::PathBuf;

use chashmap::CHashMap;
use itertools::Itertools;
use rayon::{prelude::*, ThreadPool, ThreadPoolBuilder};
use tracing::{instrument, warn};

use crate::core::formatting::printable_styled_string;
use crate::core::graph::{CommitGraph, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::git::{Commit, NonZeroOid, PatchId, Repo};
use crate::tui::{Effects, OperationType};

thread_local! {
    static REPO: RefCell<Option<Repo>> = Default::default();
}

#[derive(Debug)]
pub enum OidOrLabel {
    Oid(NonZeroOid),
    Label(String),
}

impl ToString for OidOrLabel {
    fn to_string(&self) -> String {
        match self {
            Self::Oid(oid) => oid.to_string(),
            Self::Label(label) => label.clone(),
        }
    }
}

/// A command that can be applied for either in-memory or on-disk rebases.
#[derive(Debug)]
pub enum RebaseCommand {
    /// Create a label (a reference stored in `refs/rewritten/`) pointing to the
    /// current rebase head for later use.
    CreateLabel { label_name: String },

    /// Move the rebase head to the provided label or commit.
    Reset { target: OidOrLabel },

    /// Apply the provided commit on top of the rebase head, and update the
    /// rebase head to point to the newly-applied commit.
    Pick { commit_oid: NonZeroOid },

    Merge {
        /// The original merge commit to copy the commit message from.
        commit_oid: NonZeroOid,

        /// The other commits to merge into this one. This may be a list of
        /// either OIDs or strings. This will always be one fewer commit than
        /// the number of actual parents for this commit, since we always merge
        /// into the current `HEAD` commit when rebasing.
        commits_to_merge: Vec<OidOrLabel>,
    },

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
            RebaseCommand::Reset { target } => format!("reset {}", target.to_string()),
            RebaseCommand::Pick { commit_oid } => format!("pick {}", commit_oid),
            RebaseCommand::Merge {
                commit_oid,
                commits_to_merge,
            } => format!(
                "merge -C {} {}",
                commit_oid,
                commits_to_merge
                    .iter()
                    .map(|commit_id| commit_id.to_string())
                    .join(" ")
            ),
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

/// Mutable state modified while building the rebase plan.
#[derive(Clone, Debug)]
struct BuildState {
    /// Copy of `initial_constraints` in `RebasePlanBuilder` but with any
    /// implied constraints added (such as for descendant commits).
    constraints: HashMap<NonZeroOid, HashSet<NonZeroOid>>,

    /// The set of all commits which need to be rebased. Consequently, their
    /// OIDs will change.
    commits_to_move: HashSet<NonZeroOid>,

    /// When we're rebasing a commit with OID X, its commit hash will change to
    /// some OID X', which we don't know ahead of time. However, we may still
    /// need to refer to X' as part of the rest of the rebase (such as when
    /// rebasing a tree: we have to back up to a parent node and start a new
    /// branch). Whenever we make a commit that we want to address later, we add
    /// a "label" pointing to that commit. This field is used to ensure that we
    /// don't use the same label twice.
    used_labels: HashSet<String>,

    /// When rebasing merge commits, we build a rebase plan by traversing the
    /// new commit graph in depth-first order. When we encounter a merge commit,
    /// we first have to make sure that all of its parents have been committed.
    /// (If not, then we stop this sub-traversal and wait for a later traversal
    /// to hit the same merge commit).
    merge_commit_parent_labels: HashMap<NonZeroOid, String>,

    /// Cache mapping from commit OID to the paths changed in the diff for that
    /// commit. The value is `None` if the commit doesn't have an associated
    /// diff (i.e. is a merge commit).
    touched_paths_cache: CHashMap<NonZeroOid, Option<HashSet<PathBuf>>>,
}

/// Builder for a rebase plan. Unlike regular Git rebases, a `git-branchless`
/// rebase plan can move multiple unrelated subtrees to unrelated destinations.
#[derive(Debug)]
pub struct RebasePlanBuilder<'repo, M: MergeBaseDb + 'repo> {
    repo: &'repo Repo,
    graph: &'repo CommitGraph<'repo>,
    merge_base_db: &'repo M,
    main_branch_oid: NonZeroOid,

    /// There is a mapping from from `x` to `y` if `x` must be applied before
    /// `y`.
    initial_constraints: HashMap<NonZeroOid, HashSet<NonZeroOid>>,
}

/// Can't `#[derive(Clone)]` because of the parametrized `M`, which isn't
/// clonable. (Even though we're just storing a reference to `M`, which is
/// clonable, the `derive` macro doesn't know that.)
impl<'repo, M: MergeBaseDb + 'repo> Clone for RebasePlanBuilder<'repo, M> {
    fn clone(&self) -> Self {
        Self {
            repo: self.repo,
            graph: self.graph,
            merge_base_db: self.merge_base_db,
            main_branch_oid: self.main_branch_oid,
            initial_constraints: self.initial_constraints.clone(),
        }
    }
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

    /// Calculate the patch ID for each upstream commit and compare them to the
    /// patch IDs in the to-be-rebased commits. Commits which have patch IDs
    /// which are already upstream are skipped.
    pub detect_duplicate_commits_via_patch_id: bool,
}

/// An error caused when attempting to build a rebase plan.
#[derive(Debug)]
pub enum BuildRebasePlanError {
    /// There was a cycle in the requested graph to be built.
    ConstraintCycle {
        /// The OIDs of the commits in the cycle. The first and the last OIDs are the same.
        cycle_oids: Vec<NonZeroOid>,
    },
}

impl BuildRebasePlanError {
    /// Write the error message to `out`.
    pub fn describe(&self, effects: &Effects, repo: &Repo) -> eyre::Result<()> {
        match self {
            BuildRebasePlanError::ConstraintCycle { cycle_oids } => {
                writeln!(
                    effects.get_output_stream(),
                    "This operation failed because it would introduce a cycle:"
                )?;

                let glyphs = effects.get_glyphs();
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
                        effects.get_output_stream(),
                        "{}{}{} {}",
                        char1,
                        char2,
                        char3,
                        printable_styled_string(
                            glyphs,
                            repo.friendly_describe_commit_from_oid(*oid)?
                        )?,
                    )?;
                }
            }
        }
        Ok(())
    }
}

impl<'repo, M: MergeBaseDb + 'repo> RebasePlanBuilder<'repo, M> {
    /// Constructor.
    pub fn new(
        repo: &'repo Repo,
        graph: &'repo CommitGraph,
        merge_base_db: &'repo M,
        main_branch_oid: &MainBranchOid,
    ) -> Self {
        let MainBranchOid(main_branch_oid) = main_branch_oid;
        RebasePlanBuilder {
            repo,
            graph,
            merge_base_db,
            main_branch_oid: *main_branch_oid,
            initial_constraints: Default::default(),
        }
    }

    #[instrument]
    fn make_label_name_inner(&self, state: &mut BuildState, mut preferred_name: String) -> String {
        if !state.used_labels.contains(&preferred_name) {
            state.used_labels.insert(preferred_name.clone());
            preferred_name
        } else {
            preferred_name.push('\'');
            self.make_label_name(state, preferred_name)
        }
    }

    fn make_label_name(&self, state: &mut BuildState, preferred_name: impl Into<String>) -> String {
        self.make_label_name_inner(state, preferred_name.into())
    }

    fn make_rebase_plan_for_current_commit(
        &self,
        effects: &Effects,
        state: &mut BuildState,
        previous_head_oid: NonZeroOid,
        current_commit: Commit,
        upstream_patch_ids: &HashSet<PatchId>,
        mut acc: Vec<RebaseCommand>,
    ) -> eyre::Result<Vec<RebaseCommand>> {
        let patch_already_applied_upstream = {
            if upstream_patch_ids.is_empty() {
                // Save time in the common case that there are no
                // similar-looking upstream commits, so that we don't have
                // to calculate the diff for the patch ID.
                false
            } else {
                match self.repo.get_patch_id(effects, &current_commit)? {
                    Some(current_patch_id) => upstream_patch_ids.contains(&current_patch_id),
                    None => false,
                }
            }
        };

        let acc = {
            if patch_already_applied_upstream {
                acc.push(RebaseCommand::SkipUpstreamAppliedCommit {
                    commit_oid: current_commit.get_oid(),
                });
            } else if current_commit.get_parent_count() > 1 {
                // This is a merge commit. We need to make sure that all parent
                // commits have been applied, and only then proceed with
                // applying this commit. Note that parent commits may or may not
                // be part of the set of commits to rebase (i.e. may or may not
                // be mentioned in the constraints).
                let commits_to_merge: Option<Vec<OidOrLabel>> = current_commit
                    .get_parent_oids()
                    .into_iter()
                    .filter(|parent_oid| {
                        // Don't include the parent which was the previous HEAD
                        // commit, since we need to merge into that commit.
                        *parent_oid != previous_head_oid
                    })
                    .map(|parent_oid| -> Option<OidOrLabel> {
                        let does_parent_commit_need_rebase =
                            state.commits_to_move.contains(&parent_oid);
                        if does_parent_commit_need_rebase {
                            // Since the parent commit will be rebased, it will
                            // also have a new OID, so we need to address it by
                            // the label it set when it was committed rather
                            // than by OID.
                            //
                            // If the label is not yet present, then it means
                            // that not all parents have been applied yet, so
                            // we'll postpone this commit.
                            state
                                .merge_commit_parent_labels
                                .get(&parent_oid)
                                .map(|parent_oid| OidOrLabel::Label(parent_oid.clone()))
                        } else {
                            // This parent commit was not supposed to be
                            // rebased, so its OID won't change and we can
                            // address it OID directly.
                            Some(OidOrLabel::Oid(parent_oid))
                        }
                    })
                    .collect();

                if let Some(commits_to_merge) = commits_to_merge {
                    // All parents have been committed.
                    acc.push(RebaseCommand::Merge {
                        commit_oid: current_commit.get_oid(),
                        commits_to_merge,
                    });
                } else {
                    // Wait for the caller to come back to this commit
                    // later and then proceed to any child commits.
                    return Ok(acc);
                }
            } else {
                // Normal one-parent commit (or a zero-parent commit?), just
                // rebase it and continue.
                acc.push(RebaseCommand::Pick {
                    commit_oid: current_commit.get_oid(),
                });
                acc.push(RebaseCommand::DetectEmptyCommit {
                    commit_oid: current_commit.get_oid(),
                });
            }
            acc
        };

        let child_commits: Vec<Commit> = {
            let mut child_oids: Vec<NonZeroOid> = state
                .constraints
                .entry(current_commit.get_oid())
                .or_default()
                .iter()
                .copied()
                .collect();
            child_oids.sort_unstable();
            child_oids
                .into_iter()
                .map(|child_oid| self.repo.find_commit_or_fail(child_oid))
                .try_collect()?
        };

        let acc = {
            if child_commits
                .iter()
                .any(|child_commit| child_commit.get_parent_count() > 1)
            {
                // If this commit has any merge commits as children, create a
                // label so that the child can reference this commit later for
                // merging.
                let command_num = acc.len();
                let label_name =
                    self.make_label_name(state, format!("merge-parent-{}", command_num));
                state
                    .merge_commit_parent_labels
                    .insert(current_commit.get_oid(), label_name.clone());
                let mut acc = acc;
                acc.push(RebaseCommand::CreateLabel { label_name });
                acc
            } else {
                acc
            }
        };

        if child_commits.is_empty() {
            Ok(acc)
        } else if child_commits.len() == 1 {
            let mut child_commits = child_commits;
            let only_child_commit = child_commits.pop().unwrap();
            let acc = self.make_rebase_plan_for_current_commit(
                effects,
                state,
                current_commit.get_oid(),
                only_child_commit,
                upstream_patch_ids,
                acc,
            )?;
            Ok(acc)
        } else {
            let command_num = acc.len();
            let label_name = self.make_label_name(state, format!("label-{}", command_num));
            let mut acc = acc;
            acc.push(RebaseCommand::CreateLabel {
                label_name: label_name.clone(),
            });
            for child_commit in child_commits {
                acc = self.make_rebase_plan_for_current_commit(
                    effects,
                    state,
                    current_commit.get_oid(),
                    child_commit,
                    upstream_patch_ids,
                    acc,
                )?;
                acc.push(RebaseCommand::Reset {
                    target: OidOrLabel::Label(label_name.clone()),
                });
            }
            Ok(acc)
        }
    }

    /// Generate a sequence of rebase steps that cause the subtree at `source_oid`
    /// to be rebased on top of `dest_oid`.
    pub fn move_subtree(
        &mut self,
        source_oid: NonZeroOid,
        dest_oid: NonZeroOid,
    ) -> eyre::Result<()> {
        self.initial_constraints
            .entry(dest_oid)
            .or_default()
            .insert(source_oid);
        Ok(())
    }

    #[instrument]
    fn collect_descendants(
        &self,
        effects: &Effects,
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
                self.collect_descendants(effects, acc, *child_oid)?;
            }
        }

        // Calculate the commits along the main branch to be moved if this is a
        // constraint for a main branch commit.
        let is_main = match self.graph.get(&current_oid) {
            Some(node) => node.is_main,
            None => true,
        };
        if is_main {
            // This must be a main branch commit. We need to collect its
            // descendants, which don't appear in the commit graph.
            let path = self.merge_base_db.find_path_to_merge_base(
                effects,
                self.repo,
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
                    //
                    // FIXME: this may be incorrect for multi-parent commits,
                    // since `find_path_to_merge_base` actually returns a
                    // partially-ordered set of commits.
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
    fn add_descendant_constraints(
        &self,
        effects: &Effects,
        state: &mut BuildState,
    ) -> eyre::Result<()> {
        let all_descendants_of_constrained_nodes = {
            let mut acc = Vec::new();
            for parent_oid in state.constraints.values().flatten().cloned() {
                self.collect_descendants(effects, &mut acc, parent_oid)?;
            }
            acc
        };
        for Constraint {
            parent_oid,
            child_oid,
        } in all_descendants_of_constrained_nodes
        {
            state
                .constraints
                .entry(parent_oid)
                .or_default()
                .insert(child_oid);
        }
        state.commits_to_move = state.constraints.values().flatten().copied().collect();
        Ok(())
    }

    fn check_for_cycles_helper(
        &self,
        state: &BuildState,
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
        if let Some(child_oids) = state.constraints.get(&current_oid) {
            for child_oid in child_oids.iter().sorted() {
                self.check_for_cycles_helper(state, path, *child_oid)?;
            }
        }
        Ok(())
    }

    fn check_for_cycles(
        &self,
        state: &BuildState,
        effects: &Effects,
    ) -> Result<(), BuildRebasePlanError> {
        let (_effects, _progress) = effects.start_operation(OperationType::CheckForCycles);

        // FIXME: O(n^2) algorithm.
        for oid in state.constraints.keys().sorted() {
            self.check_for_cycles_helper(state, &mut Vec::new(), *oid)?;
        }
        Ok(())
    }

    fn find_roots(&self, state: &BuildState) -> Vec<Constraint> {
        let unconstrained_nodes = {
            let mut unconstrained_nodes: HashSet<NonZeroOid> =
                state.constraints.keys().copied().collect();
            for child_oid in state.constraints.values().flatten().copied() {
                unconstrained_nodes.remove(&child_oid);
            }
            unconstrained_nodes
        };
        let mut root_edges: Vec<Constraint> = unconstrained_nodes
            .into_iter()
            .flat_map(|unconstrained_oid| {
                state.constraints[&unconstrained_oid]
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

    fn get_constraints_sorted_for_debug(
        state: &BuildState,
    ) -> Vec<(&NonZeroOid, Vec<&NonZeroOid>)> {
        state
            .constraints
            .iter()
            .map(|(k, v)| (k, v.iter().sorted().collect::<Vec<_>>()))
            .sorted()
            .collect::<Vec<_>>()
    }

    /// Create the rebase plan. Returns `None` if there were no commands in the rebase plan.
    pub fn build(
        &self,
        effects: &Effects,
        options: &BuildRebasePlanOptions,
    ) -> eyre::Result<Result<Option<RebasePlan>, BuildRebasePlanError>> {
        let BuildRebasePlanOptions {
            dump_rebase_constraints,
            dump_rebase_plan,
            detect_duplicate_commits_via_patch_id,
        } = options;
        let mut state = BuildState {
            constraints: self.initial_constraints.clone(),
            commits_to_move: Default::default(), // filled in by `add_descendant_constraints`
            used_labels: Default::default(),
            merge_commit_parent_labels: Default::default(),
            touched_paths_cache: Default::default(),
        };

        let (effects, _progress) = effects.start_operation(OperationType::BuildRebasePlan);

        if *dump_rebase_constraints {
            // For test: don't print to `effects.get_output_stream()`, as it will
            // be suppressed.
            println!(
                "Rebase constraints before adding descendants: {:#?}",
                Self::get_constraints_sorted_for_debug(&state),
            );
        }
        self.add_descendant_constraints(&effects, &mut state)?;
        if *dump_rebase_constraints {
            // For test: don't print to `effects.get_output_stream()`, as it will
            // be suppressed.
            println!(
                "Rebase constraints after adding descendants: {:#?}",
                Self::get_constraints_sorted_for_debug(&state),
            );
        }

        if let Err(err) = self.check_for_cycles(&state, &effects) {
            return Ok(Err(err));
        }

        let roots = self.find_roots(&state);
        let mut acc = vec![RebaseCommand::RegisterExtraPostRewriteHook];
        let mut first_dest_oid = None;
        for Constraint {
            parent_oid,
            child_oid,
        } in roots
        {
            first_dest_oid.get_or_insert(parent_oid);
            acc.push(RebaseCommand::Reset {
                target: OidOrLabel::Oid(parent_oid),
            });

            let upstream_patch_ids = if *detect_duplicate_commits_via_patch_id {
                let (effects, _progress) =
                    effects.start_operation(OperationType::DetectDuplicateCommits);
                self.get_upstream_patch_ids(&effects, &mut state, child_oid, parent_oid)?
            } else {
                Default::default()
            };
            acc = self.make_rebase_plan_for_current_commit(
                &effects,
                &mut state,
                parent_oid,
                self.repo.find_commit_or_fail(child_oid)?,
                &upstream_patch_ids,
                acc,
            )?;
        }

        Self::check_all_commits_included_in_rebase_plan(&state, acc.as_slice());

        let rebase_plan = first_dest_oid.map(|first_dest_oid| RebasePlan {
            first_dest_oid,
            commands: acc,
        });
        if *dump_rebase_plan {
            // For test: don't print to `effects.get_output_stream()`, as it will
            // be suppressed.
            println!("Rebase plan: {:#?}", rebase_plan);
        }
        Ok(Ok(rebase_plan))
    }

    fn check_all_commits_included_in_rebase_plan(
        state: &BuildState,
        rebase_commands: &[RebaseCommand],
    ) {
        let included_commit_oids: HashSet<NonZeroOid> = rebase_commands
            .iter()
            .filter_map(|rebase_command| match rebase_command {
                RebaseCommand::CreateLabel { label_name: _ }
                | RebaseCommand::Reset { target: _ }
                | RebaseCommand::RegisterExtraPostRewriteHook
                | RebaseCommand::DetectEmptyCommit { commit_oid: _ } => None,
                RebaseCommand::Pick { commit_oid }
                | RebaseCommand::Merge {
                    commit_oid,
                    commits_to_merge: _,
                }
                | RebaseCommand::SkipUpstreamAppliedCommit { commit_oid } => Some(*commit_oid),
            })
            .collect();
        let missing_commit_oids: HashSet<NonZeroOid> = state
            .constraints
            .values()
            .flatten()
            .copied()
            .collect::<HashSet<NonZeroOid>>()
            .sub(&included_commit_oids);
        if !missing_commit_oids.is_empty() {
            warn!(
                ?missing_commit_oids,
                "BUG? Not all commits were included in the rebase plan. \
                This means that some commits might be missing \
                after the rebase has completed."
            );
        }
    }

    #[instrument]
    fn get_upstream_patch_ids(
        &self,
        effects: &Effects,
        state: &mut BuildState,
        current_oid: NonZeroOid,
        dest_oid: NonZeroOid,
    ) -> eyre::Result<HashSet<PatchId>> {
        let merge_base_oid =
            self.merge_base_db
                .get_merge_base_oid(effects, self.repo, dest_oid, current_oid)?;
        let merge_base_oid = match merge_base_oid {
            None => return Ok(HashSet::new()),
            Some(merge_base_oid) => merge_base_oid,
        };

        let path = self.merge_base_db.find_path_to_merge_base(
            effects,
            self.repo,
            dest_oid,
            merge_base_oid,
        )?;
        let path = match path {
            None => return Ok(HashSet::new()),
            Some(path) => path,
        };

        let pool = self.make_pool(self.repo)?;

        let path = {
            let touched_commits = state
                .constraints
                .values()
                .flatten()
                .map(|oid| self.repo.find_commit(*oid))
                .flatten_ok()
                .try_collect()?;
            self.filter_path_to_merge_base_commits(effects, state, &pool, path, touched_commits)?
        };

        // FIXME: we may recalculate common patch IDs many times, should be
        // cached.
        let (effects, progress) = effects.start_operation(OperationType::GetUpstreamPatchIds);
        progress.notify_progress(0, path.len());
        let result: HashSet<PatchId> = {
            let path_oids = path
                .into_iter()
                .map(|commit| commit.get_oid())
                .collect_vec();

            pool.install(|| {
                path_oids
                    .into_par_iter()
                    .map(|commit_oid| -> eyre::Result<Option<PatchId>> {
                        REPO.with(|repo| {
                            let repo = repo.borrow();
                            let repo = repo.as_ref().expect("Could not get thread-local repo");
                            let commit = match repo.find_commit(commit_oid)? {
                                Some(commit) => commit,
                                None => return Ok(None),
                            };
                            let result = repo.get_patch_id(&effects, &commit)?;
                            Ok(result)
                        })
                    })
                    .inspect(|_| progress.notify_progress_inc(1))
                    .filter_map(|result| result.transpose())
                    .collect::<eyre::Result<HashSet<PatchId>>>()
            })?
        };
        Ok(result)
    }

    #[instrument]
    fn make_pool(&self, repo: &Repo) -> eyre::Result<ThreadPool> {
        let repo_path = repo.get_path().to_owned();
        let pool = ThreadPoolBuilder::new()
            .start_handler(move |_index| {
                REPO.with(|thread_repo| -> eyre::Result<()> {
                    let mut thread_repo = thread_repo.borrow_mut();
                    if thread_repo.is_none() {
                        *thread_repo = Some(Repo::from_dir(&repo_path)?);
                    }
                    Ok(())
                })
                .expect("Could not clone repo for thread");
            })
            .build()?;
        Ok(pool)
    }

    fn filter_path_to_merge_base_commits(
        &self,
        effects: &Effects,
        state: &mut BuildState,
        pool: &ThreadPool,
        path: Vec<Commit<'repo>>,
        touched_commits: Vec<Commit>,
    ) -> eyre::Result<Vec<Commit<'repo>>> {
        let (effects, _progress) = effects.start_operation(OperationType::FilterCommits);

        let local_touched_paths: HashSet<PathBuf> = touched_commits
            .into_iter()
            .map(|commit| self.repo.get_paths_touched_by_commit(&commit))
            .filter_map(|x| x.transpose())
            .flatten_ok()
            .try_collect()?;

        let filtered_path = {
            let (_effects, progress) = effects.start_operation(OperationType::FilterByTouchedPaths);
            progress.notify_progress(0, path.len());

            let path = path
                .into_iter()
                .map(|commit| commit.get_oid())
                .collect_vec();
            let touched_paths_cache = &state.touched_paths_cache;
            pool.install(|| {
                path.into_par_iter()
                    .map(|commit_oid| {
                        if let Some(upstream_touched_paths) = touched_paths_cache.get(&commit_oid) {
                            if Self::is_candidate_to_check_patch_id(
                                &*upstream_touched_paths,
                                &local_touched_paths,
                            ) {
                                return Ok(Some(commit_oid));
                            } else {
                                return Ok(None);
                            }
                        }

                        REPO.with(|repo| {
                            let repo = repo.borrow();
                            let repo = repo.as_ref().expect("Could not get thread-local repo");

                            let commit = match repo.find_commit(commit_oid)? {
                                Some(commit) => commit,
                                None => return Ok(None),
                            };
                            let upstream_touched_paths =
                                repo.get_paths_touched_by_commit(&commit)?;
                            let result = if Self::is_candidate_to_check_patch_id(
                                &upstream_touched_paths,
                                &local_touched_paths,
                            ) {
                                Some(commit_oid)
                            } else {
                                None
                            };
                            touched_paths_cache.insert(commit_oid, upstream_touched_paths);
                            Ok(result)
                        })
                    })
                    .inspect(|_| progress.notify_progress_inc(1))
                    .filter_map(|x| x.transpose())
                    .collect::<eyre::Result<Vec<NonZeroOid>>>()
            })?
        };
        let filtered_path = filtered_path
            .into_iter()
            .map(|commit_oid| self.repo.find_commit_or_fail(commit_oid))
            .try_collect()?;

        Ok(filtered_path)
    }

    fn is_candidate_to_check_patch_id(
        upstream_touched_paths: &Option<HashSet<PathBuf>>,
        local_touched_paths: &HashSet<PathBuf>,
    ) -> bool {
        match upstream_touched_paths {
            Some(upstream_touched_paths) => {
                // This could be more specific -- we could check to see if they are
                // exactly the same. I'm checking only if there is some intersection to
                // make sure there's not some edge-case e.g. around renames that I've
                // missed.
                !local_touched_paths.is_disjoint(upstream_touched_paths)
            }
            None => true,
        }
    }
}
