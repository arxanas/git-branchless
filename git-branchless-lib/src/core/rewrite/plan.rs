use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::ops::Sub;
use std::path::PathBuf;
use std::sync::Arc;

use chashmap::CHashMap;
use eden_dag::DagAlgorithm;
use eyre::Context;
use itertools::Itertools;
use rayon::{prelude::*, ThreadPool};
use tracing::{instrument, warn};

use crate::core::dag::{commit_set_to_vec_unsorted, union_all, CommitSet, Dag};
use crate::core::effects::{Effects, OperationType};
use crate::core::formatting::{printable_styled_string, Pluralize};
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
    Pick {
        original_commit_oid: NonZeroOid,
        commit_to_apply_oid: NonZeroOid,
    },

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
            RebaseCommand::Pick {
                original_commit_oid: _,
                commit_to_apply_oid: commit_oid,
            } => format!("pick {}", commit_oid),
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

/// A token representing that the rebase plan has been checked for validity.
#[derive(Clone, Debug)]
pub struct RebasePlanPermissions<'a> {
    build_options: &'a BuildRebasePlanOptions,
    allowed_commits: CommitSet,
}

impl<'a> RebasePlanPermissions<'a> {
    /// Construct a new `RebasePlanPermissions`.
    pub fn verify_rewrite_set(
        dag: &Dag,
        build_options: &'a BuildRebasePlanOptions,
        commits: &CommitSet,
    ) -> eyre::Result<Result<Self, BuildRebasePlanError>> {
        // This isn't necessary for correctness, but helps to produce a better
        // error message which indicates the magnitude of the issue.
        let commits = dag.query().descendants(commits.clone())?;

        let public_commits = dag.query_public_commits()?;
        if !build_options.force_rewrite_public_commits {
            let public_commits_to_move = public_commits.intersection(&commits);
            if !public_commits_to_move.is_empty()? {
                return Ok(Err(BuildRebasePlanError::MovePublicCommits {
                    public_commits_to_move,
                }));
            }
        }

        let allowed_commits = dag.query().range(
            commits.clone(),
            commits.union(&dag.query_active_heads(&public_commits, &dag.observed_commits)?),
        )?;
        Ok(Ok(RebasePlanPermissions {
            build_options,
            allowed_commits,
        }))
    }

    #[cfg(test)]
    fn omnipotent_for_test(
        dag: &Dag,
        build_options: &'a BuildRebasePlanOptions,
    ) -> eyre::Result<Self> {
        Ok(Self {
            build_options,
            allowed_commits: dag.query().all()?,
        })
    }
}

/// Represents the commits (as OIDs) that will be used to build a rebase plan.
#[derive(Clone, Debug)]
struct ConstraintGraph<'a> {
    dag: &'a Dag,
    permissions: &'a RebasePlanPermissions<'a>,

    /// This is a mapping from `x` to `y` if `x` must be applied before `y`
    inner: HashMap<NonZeroOid, HashSet<NonZeroOid>>,
}

impl<'a> ConstraintGraph<'a> {
    pub fn new(dag: &'a Dag, permissions: &'a RebasePlanPermissions) -> Self {
        Self {
            dag,
            permissions,
            inner: HashMap::new(),
        }
    }

    pub fn add_constraints(&mut self, constraints: &Vec<Constraint>) -> eyre::Result<()> {
        for constraint in constraints {
            match constraint {
                Constraint::MoveChildren {
                    parent_of_oid: _,
                    children_of_oid: _,
                } => {
                    // do nothing; these will be handled in the next pass
                }

                Constraint::MoveSubtree {
                    parent_oids,
                    child_oid,
                } => {
                    // remove previous (if any) constraints on commit
                    for commits in self.inner.values_mut() {
                        commits.remove(child_oid);
                    }

                    for parent_oid in parent_oids {
                        self.inner
                            .entry(*parent_oid)
                            .or_default()
                            .insert(*child_oid);
                    }
                }
            }
        }

        let range_heads: HashSet<&NonZeroOid> = constraints
            .iter()
            .filter_map(|c| match c {
                Constraint::MoveSubtree {
                    parent_oids: _,
                    child_oid: _,
                } => None,
                Constraint::MoveChildren {
                    parent_of_oid: _,
                    children_of_oid,
                } => Some(children_of_oid),
            })
            .collect();

        for constraint in constraints {
            match constraint {
                Constraint::MoveChildren {
                    parent_of_oid,
                    children_of_oid,
                } => {
                    let mut parent_oid = self.dag.get_only_parent_oid(*parent_of_oid)?;
                    let commits_to_move = self.commits_to_move();

                    // If parent_oid is part of another range and is itself
                    // constrained, keep looking for an unconstrained ancestor.
                    while range_heads.contains(&parent_oid) && commits_to_move.contains(&parent_oid)
                    {
                        parent_oid = self.dag.get_only_parent_oid(parent_oid)?;
                    }

                    let commits_to_move: CommitSet = commits_to_move.into_iter().collect();
                    let source_children: CommitSet = self
                        .dag
                        .query()
                        .children(CommitSet::from(*children_of_oid))?
                        .difference(&self.dag.obsolete_commits)
                        .difference(&commits_to_move);

                    for child_oid in commit_set_to_vec_unsorted(&source_children)? {
                        self.inner.entry(parent_oid).or_default().insert(child_oid);
                    }
                }

                Constraint::MoveSubtree {
                    parent_oids: _,
                    child_oid: _,
                } => {
                    // do nothing; these were handled in the first pass
                }
            }
        }

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
        let children_oids = commit_set_to_vec_unsorted(&children_oids)?;
        for child_oid in children_oids {
            if self.commits_to_move().contains(&child_oid) {
                continue;
            }
            acc.push(Constraint::MoveSubtree {
                parent_oids: vec![current_oid],
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
    fn add_descendant_constraints(&mut self, effects: &Effects) -> eyre::Result<()> {
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
            let parents = self.commits_to_move();
            progress.notify_progress(0, parents.len());
            for parent_oid in parents {
                self.collect_descendants(&visible_commits, &mut acc, parent_oid)?;
                progress.notify_progress_inc(1);
            }
            acc
        };

        self.add_constraints(&all_descendants_of_constrained_nodes)?;
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
        if let Some(child_oids) = self.commits_to_move_to(&current_oid) {
            for child_oid in child_oids.iter().sorted() {
                self.check_for_cycles_helper(path, *child_oid)?;
            }
        }
        Ok(())
    }

    fn check_for_cycles(&self, effects: &Effects) -> Result<(), BuildRebasePlanError> {
        let (effects, _progress) = effects.start_operation(OperationType::CheckForCycles);
        let _effects = effects;

        // FIXME: O(n^2) algorithm.
        for oid in self.parents().iter().sorted() {
            self.check_for_cycles_helper(&mut Vec::new(), *oid)?;
        }
        Ok(())
    }

    fn check_permissions(&self) -> eyre::Result<Result<(), BuildRebasePlanError>> {
        let RebasePlanPermissions {
            build_options: _,
            allowed_commits,
        } = &self.permissions;
        let commits_to_move: CommitSet = self.commits_to_move().into_iter().collect();
        let illegal_commits_to_move = commits_to_move.difference(allowed_commits);
        if !illegal_commits_to_move.is_empty()? {
            Ok(Err(BuildRebasePlanError::MoveIllegalCommits {
                illegal_commits_to_move,
            }))
        } else {
            Ok(Ok(()))
        }
    }

    fn find_roots(&self) -> Vec<Constraint> {
        let unconstrained_nodes = {
            let mut unconstrained_nodes: HashSet<NonZeroOid> = self.parents().into_iter().collect();
            for child_oid in self.commits_to_move() {
                unconstrained_nodes.remove(&child_oid);
            }
            unconstrained_nodes
        };
        let mut root_edges: Vec<Constraint> = unconstrained_nodes
            .into_iter()
            .filter_map(|unconstrained_oid| {
                self.commits_to_move_to(&unconstrained_oid).map(|children| {
                    children
                        .into_iter()
                        .map(move |child_oid| Constraint::MoveSubtree {
                            parent_oids: vec![unconstrained_oid],
                            child_oid,
                        })
                })
            })
            .flatten()
            .collect();
        root_edges.sort_unstable();
        root_edges
    }

    fn get_constraints_sorted_for_debug(&self) -> Vec<(NonZeroOid, Vec<NonZeroOid>)> {
        self.parents()
            .iter()
            .map(|parent_oid| {
                (
                    *parent_oid,
                    self.commits_to_move_to(parent_oid)
                        .map_or(Vec::new(), |children| {
                            children.into_iter().sorted().collect_vec()
                        }),
                )
            })
            .sorted()
            .collect_vec()
    }

    /// All of the parent (aka destination) OIDs
    pub fn parents(&self) -> Vec<NonZeroOid> {
        self.inner.keys().copied().collect_vec()
    }

    /// All of the constrained children. This is set of all commits which need
    /// to be rebased. Consequently, their OIDs will change.
    pub fn commits_to_move(&self) -> HashSet<NonZeroOid> {
        self.inner.values().flatten().copied().collect()
    }

    /// All of the constrained children being moved to a particular parent..
    pub fn commits_to_move_to(&self, parent_oid: &NonZeroOid) -> Option<Vec<NonZeroOid>> {
        self.inner
            .get(parent_oid)
            .map(|child_oids| child_oids.iter().copied().collect())
    }
}

/// Mutable state modified while building the rebase plan.
#[derive(Clone, Debug)]
struct BuildState<'repo> {
    /// Contains all of `initial_constraints` (in `RebasePlanBuilder`) plus
    /// any implied constraints added (such as for descendant commits).
    constraints: ConstraintGraph<'repo>,

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
pub struct RebasePlanBuilder<'a> {
    dag: &'a Dag,
    permissions: RebasePlanPermissions<'a>,

    /// The constraints specified by the caller.
    initial_constraints: Vec<Constraint>,

    /// Mapping of commits that should be replaced to the commits that they should be replaced
    /// with.
    replacement_commits: HashMap<NonZeroOid, NonZeroOid>,

    /// Cache mapping from commit OID to the paths changed in the diff for that
    /// commit. The value is `None` if the commit doesn't have an associated
    /// diff (i.e. is a merge commit).
    touched_paths_cache: Arc<CHashMap<NonZeroOid, Option<HashSet<PathBuf>>>>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Constraint {
    /// Indicates that the children of `children_of` should be moved on top of
    /// nearest unmoved ancestor of `parent_of`.
    MoveChildren {
        parent_of_oid: NonZeroOid,
        children_of_oid: NonZeroOid,
    },

    /// Indicates that `child` and all of its descendants should be moved on top
    /// of `parent`.
    MoveSubtree {
        parent_oids: Vec<NonZeroOid>,
        child_oid: NonZeroOid,
    },
}

/// Options used to build a rebase plan.
#[derive(Debug)]
pub struct BuildRebasePlanOptions {
    /// Force rewriting public commits, even though other users may have access
    /// to those commits.
    pub force_rewrite_public_commits: bool,

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

    /// The user was trying to move public commits.
    MovePublicCommits {
        /// The public commits which the user was trying to move.
        public_commits_to_move: CommitSet,
    },

    /// The user was trying to move commits that weren't verified before the
    /// rebase plan was built. This probably indicates a bug in the code.
    MoveIllegalCommits {
        /// The illegal commits which the user was trying to move.
        illegal_commits_to_move: CommitSet,
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
                            repo.friendly_describe_commit_from_oid(glyphs, *oid)?,
                        )?,
                    )?;
                }
            }

            BuildRebasePlanError::MovePublicCommits {
                public_commits_to_move,
            } => {
                let example_bad_commit_oid = public_commits_to_move.first()?.ok_or_else(|| {
                    eyre::eyre!("BUG: could not get OID of a public commit to move")
                })?;
                let example_bad_commit_oid = NonZeroOid::try_from(example_bad_commit_oid)?;
                let example_bad_commit = repo.find_commit_or_fail(example_bad_commit_oid)?;
                writeln!(
                    effects.get_output_stream(),
                    "\
You are trying to rewrite {}, such as: {}
It is generally not advised to rewrite public commits, because your
collaborators will have difficulty merging your changes.
Retry with -f/--force-rewrite to proceed anyways.",
                    Pluralize {
                        determiner: None,
                        amount: public_commits_to_move.count()?,
                        unit: ("public commit", "public commits")
                    },
                    printable_styled_string(
                        effects.get_glyphs(),
                        example_bad_commit.friendly_describe(effects.get_glyphs())?
                    )?,
                )?;
            }

            BuildRebasePlanError::MoveIllegalCommits {
                illegal_commits_to_move,
            } => {
                writeln!(
                    effects.get_output_stream(),
                    "\
BUG: The following commits were planned to be moved but not verified:
{:?}
This is a bug. Please report it.",
                    commit_set_to_vec_unsorted(illegal_commits_to_move)
                )?;
            }
        }
        Ok(())
    }
}

impl<'a> RebasePlanBuilder<'a> {
    /// Constructor.
    pub fn new(dag: &'a Dag, permissions: RebasePlanPermissions<'a>) -> Self {
        RebasePlanBuilder {
            dag,
            permissions,
            initial_constraints: Default::default(),
            replacement_commits: Default::default(),
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
                            state.constraints.commits_to_move().contains(&parent_oid);
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

                match commits_to_merge {
                    Some(commits_to_merge) => {
                        // All parents have been committed.
                        acc.push(RebaseCommand::Merge {
                            commit_oid: current_commit.get_oid(),
                            commits_to_merge,
                        });
                    }

                    None => {
                        // Wait for the caller to come back to this commit
                        // later and then proceed to any child commits.
                        return Ok(acc);
                    }
                }
            } else {
                // Normal one-parent commit (or a zero-parent commit?), just
                // rebase it and continue.
                let original_commit_oid = current_commit.get_oid();
                let commit_oid = match self.replacement_commits.get(&original_commit_oid) {
                    Some(replacement_oid) => *replacement_oid,
                    None => original_commit_oid,
                };
                acc.push(RebaseCommand::Pick {
                    original_commit_oid,
                    commit_to_apply_oid: commit_oid,
                });
                acc.push(RebaseCommand::DetectEmptyCommit {
                    commit_oid: current_commit.get_oid(),
                });
            }
            acc
        };

        let child_commits: Vec<Commit> = state
            .constraints
            .commits_to_move_to(&current_commit.get_oid())
            .map_or(Ok(Vec::new()), |mut child_oids| {
                child_oids.sort_unstable();
                child_oids
                    .into_iter()
                    .map(|child_oid| repo.find_commit_or_fail(child_oid))
                    .try_collect()
            })?;

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
    /// to be rebased on top of `dest_oids`.
    pub fn move_subtree(
        &mut self,
        source_oid: NonZeroOid,
        dest_oids: Vec<NonZeroOid>,
    ) -> eyre::Result<()> {
        assert!(!dest_oids.is_empty());
        self.initial_constraints.push(Constraint::MoveSubtree {
            parent_oids: dest_oids,
            child_oid: source_oid,
        });
        Ok(())
    }

    /// Generate a sequence of rebase steps that cause the commit at
    /// `source_oid` to be rebased on top of `dest_oid`, and for the descendants
    /// of `source_oid` to be rebased on top of its parent.
    pub fn move_commit(
        &mut self,
        source_oid: NonZeroOid,
        dest_oid: NonZeroOid,
    ) -> eyre::Result<()> {
        self.move_range(source_oid, source_oid, dest_oid)
    }

    /// Generate a sequence of rebase steps that cause the range from
    /// `source_oid` to `end_oid` to be rebased on top of `dest_oid`, and for
    /// the descendants of `end_oid` to be rebased on top of the parent of
    /// `source_oid`.
    pub fn move_range(
        &mut self,
        source_oid: NonZeroOid,
        end_oid: NonZeroOid,
        dest_oid: NonZeroOid,
    ) -> eyre::Result<()> {
        self.initial_constraints.push(Constraint::MoveSubtree {
            parent_oids: vec![dest_oid],
            child_oid: source_oid,
        });
        self.initial_constraints.push(Constraint::MoveChildren {
            parent_of_oid: source_oid,
            children_of_oid: end_oid,
        });
        Ok(())
    }

    /// Instruct the rebase planner to replace the commit at `original_oid` with the commit at
    /// `replacement_oid`.
    pub fn replace_commit(
        &mut self,
        original_oid: NonZeroOid,
        replacement_oid: NonZeroOid,
    ) -> eyre::Result<()> {
        if self.replacement_commits.contains_key(&original_oid) {
            eyre::bail!(
                "Attempting to rewrite commit {}. Refusing to replace a commit twice.",
                original_oid
            );
        }
        self.replacement_commits
            .insert(original_oid, replacement_oid);
        Ok(())
    }

    /// Create the rebase plan. Returns `None` if there were no commands in the rebase plan.
    pub fn build(
        &self,
        effects: &Effects,
        pool: &ThreadPool,
        repo_pool: &ResourcePool<RepoResource>,
    ) -> eyre::Result<Result<Option<RebasePlan>, BuildRebasePlanError>> {
        let mut constraints = ConstraintGraph::new(self.dag, &self.permissions);
        constraints.add_constraints(&self.initial_constraints)?;
        let mut state = BuildState {
            constraints,
            used_labels: Default::default(),
            merge_commit_parent_labels: Default::default(),
        };

        let (effects, _progress) = effects.start_operation(OperationType::BuildRebasePlan);

        let BuildRebasePlanOptions {
            force_rewrite_public_commits: _,
            dump_rebase_constraints,
            dump_rebase_plan,
            detect_duplicate_commits_via_patch_id,
        } = self.permissions.build_options;
        if *dump_rebase_constraints {
            // For test: don't print to `effects.get_output_stream()`, as it will
            // be suppressed.
            println!(
                "Rebase constraints before adding descendants: {:#?}",
                state.constraints.get_constraints_sorted_for_debug(),
            );
        }
        state.constraints.add_descendant_constraints(&effects)?;
        if *dump_rebase_constraints {
            // For test: don't print to `effects.get_output_stream()`, as it will
            // be suppressed.
            println!(
                "Rebase constraints after adding descendants: {:#?}",
                state.constraints.get_constraints_sorted_for_debug(),
            );
        }

        if let Err(err) = state.constraints.check_for_cycles(&effects) {
            return Ok(Err(err));
        }
        if let Err(err) = state.constraints.check_permissions()? {
            return Ok(Err(err));
        }

        let repo = repo_pool.try_create()?;
        let roots = state.constraints.find_roots();
        let mut acc = Vec::new();
        let mut first_dest_oid = None;
        for constraint in roots {
            let (parent_oids, child_oid) = match constraint {
                Constraint::MoveSubtree {
                    parent_oids,
                    child_oid,
                } => (parent_oids, child_oid),
                Constraint::MoveChildren {
                    parent_of_oid: _,
                    children_of_oid: _,
                } => eyre::bail!("BUG: Invalid constraint encountered while preparing rebase plan.\nThis should be unreachable."),
            };
            let first_parent_oid = *parent_oids.first().unwrap();
            first_dest_oid.get_or_insert(first_parent_oid);
            acc.push(RebaseCommand::Reset {
                target: OidOrLabel::Oid(first_parent_oid),
            });

            let upstream_patch_ids = if *detect_duplicate_commits_via_patch_id {
                let (effects, _progress) =
                    effects.start_operation(OperationType::DetectDuplicateCommits);
                self.get_upstream_patch_ids(
                    &effects,
                    pool,
                    repo_pool,
                    &repo,
                    &mut state,
                    child_oid,
                    &parent_oids,
                )?
            } else {
                Default::default()
            };
            acc = self.make_rebase_plan_for_current_commit(
                &effects,
                &repo,
                &mut state,
                first_parent_oid,
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
                RebaseCommand::Pick {
                    original_commit_oid: _,
                    commit_to_apply_oid: commit_oid,
                }
                | RebaseCommand::Merge {
                    commit_oid,
                    commits_to_merge: _,
                }
                | RebaseCommand::SkipUpstreamAppliedCommit { commit_oid } => Some(*commit_oid),
            })
            .collect();
        let missing_commit_oids = state
            .constraints
            .commits_to_move()
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

    /// Get the patch IDs for commits between `current_oid` and `dest_oids`,
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
        dest_oids: &[NonZeroOid],
    ) -> eyre::Result<HashSet<PatchId>> {
        let merge_base_oids: Vec<CommitSet> = dest_oids
            .iter()
            .map(|dest_oid| {
                let commit_set: CommitSet = [current_oid, *dest_oid].into_iter().collect();
                self.dag.query().gca_all(commit_set)
            })
            .try_collect()?;
        let merge_base_oids = union_all(&merge_base_oids);

        let touched_commit_oids: Vec<NonZeroOid> =
            state.constraints.commits_to_move().into_iter().collect();

        let path = {
            let (effects, _progress) = effects.start_operation(OperationType::WalkCommits);
            let _effects = effects;
            self.dag
                .query()
                .range(merge_base_oids, dest_oids.iter().copied().collect())
                .wrap_err("Calculating upstream commits")?
        };

        let path = {
            self.filter_path_to_merge_base_commits(
                effects,
                pool,
                repo_pool,
                repo,
                path,
                touched_commit_oids,
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
        repo: &'a Repo,
        path: CommitSet,
        touched_commit_oids: Vec<NonZeroOid>,
    ) -> eyre::Result<Vec<Commit<'a>>> {
        let (effects, _progress) = effects.start_operation(OperationType::FilterCommits);
        let path = commit_set_to_vec_unsorted(&path)?;

        let touched_commits: Vec<Commit> = touched_commit_oids
            .into_iter()
            .map(|oid| repo.find_commit(oid))
            .flatten_ok()
            .try_collect()?;
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
    use std::time::SystemTime;

    use rayon::ThreadPoolBuilder;

    use crate::core::check_out::CheckOutCommitOptions;
    use crate::core::eventlog::{EventLogDb, EventReplayer};
    use crate::core::formatting::Glyphs;
    use crate::core::repo_ext::RepoExt;
    use crate::core::rewrite::{
        execute_rebase_plan, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    };
    use crate::testing::{make_git, Git};

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

        let build_options = BuildRebasePlanOptions {
            force_rewrite_public_commits: true,
            dump_rebase_constraints: false,
            dump_rebase_plan: false,
            detect_duplicate_commits_via_patch_id: true,
        };
        let permissions = RebasePlanPermissions::omnipotent_for_test(&dag, &build_options)?;
        let pool = ThreadPoolBuilder::new().build()?;
        let repo_pool = RepoPool::new(RepoResource {
            repo: Mutex::new(repo.try_clone()?),
        });
        let mut builder = RebasePlanBuilder::new(&dag, permissions);
        let builder2 = builder.clone();
        builder.move_subtree(test3_oid, vec![test1_oid])?;
        let result = builder.build(&effects, &pool, &repo_pool)?;
        let result = result.unwrap();
        let _ignored: Option<RebasePlan> = result;
        assert!(builder.touched_paths_cache.contains_key(&test1_oid));
        assert!(builder2.touched_paths_cache.contains_key(&test1_oid));

        Ok(())
    }

    #[test]
    fn test_plan_moving_subtree_again_overrides_previous_move() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        let test2_oid = git.commit_file("test2", 2)?;
        git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_subtree(test4_oid, vec![test1_oid])?;
            builder.move_subtree(test4_oid, vec![test2_oid])?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | @ 8556cef create test4.txt
        |
        o 70deb1e create test3.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_subtree_within_moved_subtree() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_subtree(test3_oid, vec![test1_oid])?;
            builder.move_subtree(test4_oid, vec![test1_oid])?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | @ 8e71a64 create test4.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_subtree_within_moved_subtree_in_other_order() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_subtree(test4_oid, vec![test1_oid])?;
            builder.move_subtree(test3_oid, vec![test1_oid])?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | @ 8e71a64 create test4.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_commit_again_overrides_previous_move() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        let test2_oid = git.commit_file("test2", 2)?;
        git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_commit(test4_oid, test1_oid)?;
            builder.move_commit(test4_oid, test2_oid)?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | @ 8556cef create test4.txt
        |
        o 70deb1e create test3.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_nonconsecutive_commits() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        git.commit_file("test4", 4)?;
        let test5_oid = git.commit_file("test5", 5)?;
        git.commit_file("test6", 6)?;

        git.run(&["smartlog"])?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_commit(test3_oid, test1_oid)?;
            builder.move_commit(test5_oid, test1_oid)?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 4ec3989 create test5.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        |
        o 8556cef create test4.txt
        |
        @ 0a34830 create test6.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_consecutive_commits() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;
        git.commit_file("test5", 5)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_commit(test3_oid, test1_oid)?;
            builder.move_commit(test4_oid, test1_oid)?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 8e71a64 create test4.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        |
        @ f26f28e create test5.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_consecutive_commits_in_other_order() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;
        git.commit_file("test5", 5)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_commit(test4_oid, test1_oid)?;
            builder.move_commit(test3_oid, test1_oid)?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 8e71a64 create test4.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        |
        @ f26f28e create test5.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_commit_and_one_child_leaves_other_child() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        git.detach_head()?;
        let test4_oid = git.commit_file("test4", 4)?;
        git.run(&["checkout", &test3_oid.to_string()])?;
        git.commit_file("test5", 5)?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        o 70deb1e create test3.txt
        |\
        | o 355e173 create test4.txt
        |
        @ 9ea1b36 create test5.txt
        "###);

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_commit(test3_oid, test1_oid)?;
            builder.move_commit(test4_oid, test1_oid)?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 8e71a64 create test4.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        |
        @ f26f28e create test5.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_commit_add_then_giving_it_a_child() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        git.commit_file("test4", 4)?;
        let test5_oid = git.commit_file("test5", 5)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_commit(test3_oid, test1_oid)?;
            builder.move_commit(test5_oid, test3_oid)?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o ad2c2fc create test3.txt
        | |
        | @ ee4aebf create test5.txt
        |
        o 96d1c37 create test2.txt
        |
        o 8556cef create test4.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_range_again_overrides_previous_move() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        let test2_oid = git.commit_file("test2", 2)?;
        git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;
        let test5_oid = git.commit_file("test5", 5)?;
        git.commit_file("test6", 6)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_range(test4_oid, test5_oid, test1_oid)?;
            builder.move_range(test4_oid, test5_oid, test2_oid)?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | o 8556cef create test4.txt
        | |
        | o 566236a create test5.txt
        |
        o 70deb1e create test3.txt
        |
        @ 35928ae create test6.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_range_and_then_partial_beginning_range_again() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;
        let test5_oid = git.commit_file("test5", 5)?;
        git.commit_file("test6", 6)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_range(test3_oid, test5_oid, test1_oid)?;
            builder.move_range(test3_oid, test4_oid, test1_oid)?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;

        // FIXME This output is correct mathematically, but feels like it should
        // be incorrect. What *should* we be doing if the user moves 2 ranges w/
        // the same source/root oid but different end oids??
        //
        // NOTE See also the next test for the other case: where the end of the
        // range is moved again. That *does* feel correct.
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o ad2c2fc create test3.txt
        | |
        | o 2b45b52 create test4.txt
        |
        o 96d1c37 create test2.txt
        |\
        | @ 99a62a3 create test6.txt
        |
        o f26f28e create test5.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_range_and_then_partial_ending_range_again() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;
        let test5_oid = git.commit_file("test5", 5)?;
        git.commit_file("test6", 6)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_range(test3_oid, test5_oid, test1_oid)?;
            builder.move_range(test4_oid, test5_oid, test1_oid)?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 8e71a64 create test4.txt
        | |
        | o 3d57d30 create test5.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        |
        @ 99a62a3 create test6.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_subtree_and_commit_within_subtree() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        let _test2_oid = git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;
        let _test5_oid = git.commit_file("test5", 5)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_subtree(test3_oid, vec![test1_oid])?;
            builder.move_commit(test4_oid, test1_oid)?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 8e71a64 create test4.txt
        |\
        | o ad2c2fc create test3.txt
        | |
        | @ ee4aebf create test5.txt
        |
        o 96d1c37 create test2.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_subtree_and_then_its_parent_commit() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        let _test2_oid = git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;
        let _test5_oid = git.commit_file("test5", 5)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_subtree(test4_oid, vec![test1_oid])?;
            builder.move_commit(test3_oid, test1_oid)?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |\
        | o 8e71a64 create test4.txt
        | |
        | @ 3d57d30 create test5.txt
        |\
        | o ad2c2fc create test3.txt
        |
        o 96d1c37 create test2.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_subtree_to_descendant_of_itself() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let _test1_oid = git.commit_file("test1", 1)?;
        let test2_oid = git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        let _test4_oid = git.commit_file("test4", 4)?;
        let test5_oid = git.commit_file("test5", 5)?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_subtree(test3_oid, vec![test5_oid])?;
            builder.move_subtree(test5_oid, vec![test2_oid])?;
            Ok(())
        })?;

        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |
        @ f26f28e create test5.txt
        |
        o 2b42d9c create test3.txt
        |
        o c533a65 create test4.txt
        "###);

        Ok(())
    }

    #[test]
    fn test_plan_moving_subtree_with_merge_commit() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;
        git.detach_head()?;
        let _test1_oid = git.commit_file("test1", 1)?;
        let test2_oid = git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;
        git.run(&["checkout", "HEAD~"])?;
        let _test5_oid = git.commit_file("test5", 5)?;
        git.run(&["merge", &test4_oid.to_string()])?;
        git.run(&["checkout", "HEAD~"])?;

        create_and_execute_plan(&git, move |builder: &mut RebasePlanBuilder| {
            builder.move_subtree(test3_oid, vec![test2_oid])?;
            Ok(())
        })?;

        // FIXME: this is wrong, the merge commit should be moved as well.
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |
        o 62fc20d create test1.txt
        |
        o 96d1c37 create test2.txt
        |\
        | o b8f27a8 create test3.txt
        | |\
        | | @ 2b47b50 create test5.txt
        | |
        | o 22cf458 create test4.txt
        |
        x 70deb1e (rewritten as b8f27a86) create test3.txt
        |\
        | x 355e173 (rewritten as 22cf4586) create test4.txt
        | |
        | o 8fb706a Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
        |
        x 9ea1b36 (rewritten as 2b47b505) create test5.txt
        |
        o 8fb706a Merge commit '355e173bf9c5d2efac2e451da0cdad3fb82b869a' into HEAD
        hint: there is 1 abandoned commit in your commit graph
        hint: to fix this, run: git restack
        hint: disable this hint by running: git config --global branchless.hint.smartlogFixAbandoned false
        "###);

        Ok(())
    }

    /// Helper function to handle the boilerplate involved in creating, building
    /// and executing the rebase plan.
    fn create_and_execute_plan(
        git: &Git,
        builder_callback_fn: impl Fn(&mut RebasePlanBuilder) -> eyre::Result<()>,
    ) -> eyre::Result<()> {
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

        let build_options = BuildRebasePlanOptions {
            force_rewrite_public_commits: false,
            dump_rebase_constraints: false,
            dump_rebase_plan: false,
            detect_duplicate_commits_via_patch_id: true,
        };
        let permissions = RebasePlanPermissions::omnipotent_for_test(&dag, &build_options)?;
        let mut builder = RebasePlanBuilder::new(&dag, permissions);

        builder_callback_fn(&mut builder)?;

        let build_result = builder.build(&effects, &pool, &repo_pool)?;

        let rebase_plan = match build_result {
            Ok(None) => return Ok(()),
            Ok(Some(rebase_plan)) => rebase_plan,
            Err(rebase_plan_error) => {
                eyre::bail!("Error building rebase plan: {:#?}", rebase_plan_error)
            }
        };

        let now = SystemTime::UNIX_EPOCH;
        let options = ExecuteRebasePlanOptions {
            now,
            event_tx_id: event_log_db.make_transaction_id(now, "test plan")?,
            preserve_timestamps: false,
            force_in_memory: true,
            force_on_disk: false,
            resolve_merge_conflicts: false,
            check_out_commit_options: CheckOutCommitOptions {
                additional_args: Default::default(),
                render_smartlog: false,
            },
        };
        let git_run_info = git.get_git_run_info();
        let result = execute_rebase_plan(
            &effects,
            &git_run_info,
            &repo,
            &event_log_db,
            &rebase_plan,
            &options,
        )?;
        assert!(matches!(
            result,
            ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ }
        ));

        Ok(())
    }
}
