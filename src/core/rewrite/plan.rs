use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::ops::Sub;
use std::path::PathBuf;
use std::sync::Arc;

use chashmap::CHashMap;
use eden_dag::DagAlgorithm;
use itertools::Itertools;
use rayon::{prelude::*, ThreadPool};
use tracing::{instrument, warn};

use crate::core::dag::{commit_set_to_vec, CommitSet, Dag};
use crate::core::effects::{Effects, OperationType};
use crate::core::formatting::printable_styled_string;
use crate::core::rewrite::{RepoPool, RepoResource};
use crate::core::task::ResourcePool;
use crate::git::{Commit, NonZeroOid, PatchId, Repo};

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
}

/// Builder for a rebase plan. Unlike regular Git rebases, a `git-branchless`
/// rebase plan can move multiple unrelated subtrees to unrelated destinations.
#[derive(Clone, Debug)]
pub struct RebasePlanBuilder<'repo> {
    dag: &'repo Dag,

    /// There is a mapping from from `x` to `y` if `x` must be applied before
    /// `y`.
    initial_constraints: HashMap<NonZeroOid, HashSet<NonZeroOid>>,

    /// Cache mapping from commit OID to the paths changed in the diff for that
    /// commit. The value is `None` if the commit doesn't have an associated
    /// diff (i.e. is a merge commit).
    touched_paths_cache: Arc<CHashMap<NonZeroOid, Option<HashSet<PathBuf>>>>,
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

impl<'repo> RebasePlanBuilder<'repo> {
    /// Constructor.
    pub fn new(dag: &'repo Dag) -> Self {
        RebasePlanBuilder {
            dag,
            initial_constraints: Default::default(),
            touched_paths_cache: Default::default(),
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
        repo: &Repo,
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
                match repo.get_patch_id(effects, &current_commit)? {
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
                .map(|child_oid| repo.find_commit_or_fail(child_oid))
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
                repo,
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
                    repo,
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
        visible_commits: &CommitSet,
        acc: &mut Vec<Constraint>,
        current_oid: NonZeroOid,
    ) -> eyre::Result<()> {
        let children_oids = self
            .dag
            .query()
            .children(CommitSet::from(current_oid))?
            .intersection(visible_commits);
        let children_oids = commit_set_to_vec(&children_oids)?;
        for child_oid in children_oids {
            acc.push(Constraint {
                parent_oid: current_oid,
                child_oid,
            });
            self.collect_descendants(visible_commits, acc, child_oid)?;
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
        let (effects, progress) = effects.start_operation(OperationType::ConstrainCommits);
        let _effects = effects;

        let all_descendants_of_constrained_nodes = {
            let public_commits = self.dag.query_public_commits()?;
            let active_heads = self.dag.query_active_heads(
                &public_commits,
                &self
                    .dag
                    .observed_commits
                    .difference(&self.dag.obsolete_commits),
            )?;
            let visible_commits = self.dag.query().ancestors(active_heads)?;

            let mut acc = Vec::new();
            let parents = state.constraints.values().flatten().cloned().collect_vec();
            progress.notify_progress(0, parents.len());
            for parent_oid in parents {
                self.collect_descendants(&visible_commits, &mut acc, parent_oid)?;
                progress.notify_progress_inc(1);
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
        pool: &ThreadPool,
        repo_pool: &ResourcePool<RepoResource>,
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

        let repo = repo_pool.try_create()?;
        let roots = self.find_roots(&state);
        let mut acc = Vec::new();
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
                self.get_upstream_patch_ids(
                    &effects, pool, repo_pool, &repo, &mut state, child_oid, parent_oid,
                )?
            } else {
                Default::default()
            };
            acc = self.make_rebase_plan_for_current_commit(
                &effects,
                &repo,
                &mut state,
                parent_oid,
                repo.find_commit_or_fail(child_oid)?,
                &upstream_patch_ids,
                acc,
            )?;
        }
        acc.push(RebaseCommand::RegisterExtraPostRewriteHook);

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

    /// Get the patch IDs for commits between `current_oid` and `dest_oid`,
    /// filtered to only the commits which might have the same patch ID as a
    /// commit being rebased.
    #[instrument]
    fn get_upstream_patch_ids(
        &self,
        effects: &Effects,
        pool: &ThreadPool,
        repo_pool: &RepoPool,
        repo: &Repo,
        state: &mut BuildState,
        current_oid: NonZeroOid,
        dest_oid: NonZeroOid,
    ) -> eyre::Result<HashSet<PatchId>> {
        let merge_base_oid =
            self.dag
                .get_one_merge_base_oid(effects, repo, dest_oid, current_oid)?;
        let merge_base_oid = match merge_base_oid {
            None => return Ok(HashSet::new()),
            Some(merge_base_oid) => merge_base_oid,
        };

        let touched_commits = {
            let (effects, _progress) = effects.start_operation(OperationType::ConstrainCommits);
            let _effects = effects;
            state
                .constraints
                .values()
                .flatten()
                .map(|oid| repo.find_commit(*oid))
                .flatten_ok()
                .try_collect()?
        };

        let path = {
            let (effects, _progress) = effects.start_operation(OperationType::WalkCommits);

            let path =
                self.dag
                    .find_path_to_merge_base(&effects, repo, dest_oid, merge_base_oid)?;
            match path {
                None => return Ok(HashSet::new()),
                Some(path) => path,
            }
        };

        let path = {
            self.filter_path_to_merge_base_commits(
                effects,
                pool,
                repo_pool,
                repo,
                path,
                touched_commits,
            )?
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
                        let repo = repo_pool.try_create()?;
                        let commit = match repo.find_commit(commit_oid)? {
                            Some(commit) => commit,
                            None => return Ok(None),
                        };
                        let result = repo.get_patch_id(&effects, &commit)?;
                        Ok(result)
                    })
                    .inspect(|_| progress.notify_progress_inc(1))
                    .filter_map(|result| result.transpose())
                    .collect::<eyre::Result<HashSet<PatchId>>>()
            })?
        };
        Ok(result)
    }

    fn filter_path_to_merge_base_commits(
        &self,
        effects: &Effects,
        pool: &ThreadPool,
        repo_pool: &RepoPool,
        repo: &'repo Repo,
        path: Vec<Commit<'repo>>,
        touched_commits: Vec<Commit>,
    ) -> eyre::Result<Vec<Commit<'repo>>> {
        let (effects, _progress) = effects.start_operation(OperationType::FilterCommits);

        let local_touched_paths: Vec<HashSet<PathBuf>> = touched_commits
            .into_iter()
            .map(|commit| repo.get_paths_touched_by_commit(&commit))
            .filter_map(|x| x.transpose())
            .try_collect()?;

        let filtered_path = {
            enum CacheLookupResult<T, U> {
                Cached(T),
                NotCached(U),
            }
            let path = path
                .into_iter()
                .map(|commit| commit.get_oid())
                .collect_vec();
            let touched_paths_cache = &self.touched_paths_cache;
            // Check cache before distributing work to threads.
            let path: Vec<CacheLookupResult<Option<NonZeroOid>, NonZeroOid>> = {
                let (effects, progress) = effects.start_operation(OperationType::ReadingFromCache);
                let _effects = effects;
                progress.notify_progress(0, path.len());
                if touched_paths_cache.is_empty() {
                    // Fast path for when the cache hasn't been populated.
                    path.into_iter().map(CacheLookupResult::NotCached).collect()
                } else {
                    path.into_iter()
                        .map(|commit_oid| match touched_paths_cache.get(&commit_oid) {
                            Some(upstream_touched_paths) => {
                                if Self::should_check_patch_id(
                                    &*upstream_touched_paths,
                                    &local_touched_paths,
                                ) {
                                    CacheLookupResult::Cached(Some(commit_oid))
                                } else {
                                    CacheLookupResult::Cached(None)
                                }
                            }
                            None => CacheLookupResult::NotCached(commit_oid),
                        })
                        .inspect(|_| progress.notify_progress_inc(1))
                        .collect()
                }
            };

            let (_effects, progress) = effects.start_operation(OperationType::FilterByTouchedPaths);
            progress.notify_progress(0, path.len());

            pool.install(|| {
                path.into_par_iter()
                    .map(|commit_oid| {
                        let commit_oid = match commit_oid {
                            CacheLookupResult::Cached(result) => return Ok(result),
                            CacheLookupResult::NotCached(commit_oid) => commit_oid,
                        };

                        let repo = repo_pool.try_create()?;
                        let commit = match repo.find_commit(commit_oid)? {
                            Some(commit) => commit,
                            None => return Ok(None),
                        };
                        let upstream_touched_paths = repo.get_paths_touched_by_commit(&commit)?;
                        let result = if Self::should_check_patch_id(
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
                    .inspect(|_| progress.notify_progress_inc(1))
                    .filter_map(|x| x.transpose())
                    .collect::<eyre::Result<Vec<NonZeroOid>>>()
            })?
        };
        let filtered_path = filtered_path
            .into_iter()
            .map(|commit_oid| repo.find_commit_or_fail(commit_oid))
            .try_collect()?;

        Ok(filtered_path)
    }

    fn should_check_patch_id(
        upstream_touched_paths: &Option<HashSet<PathBuf>>,
        local_touched_paths: &[HashSet<PathBuf>],
    ) -> bool {
        match upstream_touched_paths {
            Some(upstream_touched_paths) => {
                // It's possible that the same commit was applied after a parent
                // commit renamed a certain path. In that case, this check won't
                // trigger. We'll rely on the empty-commit check after the
                // commit has been made to deduplicate the commit in that case.
                // FIXME: this code path could be optimized further.
                local_touched_paths.iter().contains(upstream_touched_paths)
            }
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use rayon::ThreadPoolBuilder;

    use crate::core::eventlog::{EventLogDb, EventReplayer};
    use crate::core::formatting::Glyphs;
    use crate::testing::make_git;

    use super::*;

    #[test]
    fn test_cache_shared_between_builders() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;

        let effects = Effects::new_suppress_for_test(Glyphs::text());
        let repo = git.get_repo()?;
        let conn = repo.get_db_conn()?;
        let event_log_db = EventLogDb::new(&conn)?;
        let event_replayer = EventReplayer::from_event_log_db(&effects, &repo, &event_log_db)?;
        let event_cursor = event_replayer.make_default_cursor();
        let references_snapshot = repo.get_references_snapshot()?;
        let dag = Dag::open_and_sync(
            &effects,
            &repo,
            &event_replayer,
            event_cursor,
            &references_snapshot,
        )?;

        let pool = ThreadPoolBuilder::new().build()?;
        let repo_pool = RepoPool::new(RepoResource {
            repo: Mutex::new(repo.try_clone()?),
        });
        let mut builder = RebasePlanBuilder::new(&dag);
        let builder2 = builder.clone();
        builder.move_subtree(test3_oid, test1_oid)?;
        let result = builder.build(
            &effects,
            &pool,
            &repo_pool,
            &BuildRebasePlanOptions {
                dump_rebase_constraints: false,
                dump_rebase_plan: false,
                detect_duplicate_commits_via_patch_id: true,
            },
        )?;
        let result = result.unwrap();
        let _ignored: Option<RebasePlan> = result;
        assert!(builder.touched_paths_cache.contains_key(&test1_oid));
        assert!(builder2.touched_paths_cache.contains_key(&test1_oid));

        Ok(())
    }
}
