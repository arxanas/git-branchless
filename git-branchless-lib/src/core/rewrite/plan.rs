use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Write};
use std::ops::Sub;
use std::path::PathBuf;
use std::sync::Arc;

use chashmap::CHashMap;
use eyre::Context;
use itertools::Itertools;
use rayon::{prelude::*, ThreadPool};
use tracing::{instrument, warn};

use crate::core::dag::{sorted_commit_set, union_all, CommitSet, Dag};
use crate::core::effects::{Effects, OperationType, WithProgress};
use crate::core::formatting::Pluralize;
use crate::core::rewrite::{RepoPool, RepoResource};
use crate::core::task::ResourcePool;
use crate::git::{Commit, NonZeroOid, PatchId, Repo};

/// Represents the target for certain [`RebaseCommand`]s.
#[derive(Clone, Debug)]
pub enum OidOrLabel {
    /// A commit hash to check out directly.
    Oid(NonZeroOid),

    /// A label created previously with [`RebaseCommand::CreateLabel`].
    Label(String),
}

impl Display for OidOrLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OidOrLabel::Oid(oid) => write!(f, "{oid}"),
            OidOrLabel::Label(label) => write!(f, "{label}"),
        }
    }
}

/// A command that can be applied for either in-memory or on-disk rebases.
#[derive(Debug)]
pub enum RebaseCommand {
    /// Create a label (a reference stored in `refs/rewritten/`) pointing to the
    /// current rebase head for later use.
    CreateLabel {
        /// The new label name.
        label_name: String,
    },

    /// Move the rebase head to the provided label or commit.
    Reset {
        /// The target to check out.
        target: OidOrLabel,
    },

    /// Apply the provided commits on top of the rebase head, and update the
    /// rebase head to point to the newly-applied commit.
    ///
    /// If multiple commits are provided, they will be squashed into a single
    /// commit.
    Pick {
        /// The original commit, which contains the relevant metadata such as
        /// the commit message.
        original_commit_oid: NonZeroOid,

        /// The commits whose patches should be applied to the rebase head.
        ///  - This will be different from [`original_commit_oid`] when a commit
        ///    is being replaced.
        ///  - If this is a single commit, then the rebase will perform a normal
        ///    `pick`.
        ///  - If this is multiple commits, they will all be squashed into a
        ///    single commit, reusing the metadata (message, author, timestamps,
        ///    etc) from `original_commit_oid`.
        commits_to_apply_oids: Vec<NonZeroOid>,
    },

    /// Merge two or more parent commits.
    Merge {
        /// The original merge commit to copy the commit contents from.
        commit_oid: NonZeroOid,

        /// The other commits to merge into this one. This may be a list of
        /// either OIDs or strings. This will always be one fewer commit than
        /// the number of actual parents for this commit, since we always merge
        /// into the current `HEAD` commit when rebasing.
        commits_to_merge: Vec<OidOrLabel>,
    },

    /// When rebasing `commit_oid`, apply it by replacing its commit metadata
    /// and tree with that of `replacement_commit_oid`, and by replacing its
    /// parents with `parents`. No patches will be computed or applied, so this
    /// operation is guaranteed to never cause a merge conflict.
    Replace {
        /// The commit to be replaced. It should be part of another rebase command, or else we will
        /// never encounter it for replacement.
        commit_oid: NonZeroOid,

        /// The replacement commit whose metadata and tree we'll use.
        replacement_commit_oid: NonZeroOid,

        /// The new parents for the replaced commit.
        parents: Vec<OidOrLabel>,
    },

    /// Pause the rebase, to be resumed later. Only supported in on-disk
    /// rebases.
    Break,

    /// On-disk rebases only. Register that we want to run cleanup at the end of
    /// the rebase, during the `post-rewrite` hook.
    RegisterExtraPostRewriteHook,

    /// Determine if the current commit is empty. If so, reset the rebase head
    /// to its parent and record that it was empty in the `rewritten-list`.
    DetectEmptyCommit {
        /// The original commit. If the new commit is empty, then the original
        /// commit will be recorded as skipped.
        commit_oid: NonZeroOid,
    },

    /// The commit that would have been applied to the rebase head was already
    /// applied upstream. Skip it and record it in the `rewritten-list`.
    SkipUpstreamAppliedCommit {
        /// The original commit, which will be recorded as skipped.
        commit_oid: NonZeroOid,
    },
}

impl RebaseCommand {
    /// Convert the command to a string that's used in the `git rebase` plan
    /// format.
    pub fn to_rebase_command(&self) -> String {
        match self {
            RebaseCommand::CreateLabel { label_name } => format!("label {label_name}"),
            RebaseCommand::Reset { target } => format!("reset {target}"),
            RebaseCommand::Pick {
                original_commit_oid: _,
                commits_to_apply_oids,
            } => match commits_to_apply_oids.as_slice() {
                [] => String::new(),
                [commit_oid] => format!("pick {commit_oid}"),
                [..] => unimplemented!("On-disk fixups are not yet supported"),
            },
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
            RebaseCommand::Replace {
                commit_oid: _,
                replacement_commit_oid,
                parents,
            } => {
                let parents = parents
                    .iter()
                    .map(|parent| match parent {
                        OidOrLabel::Oid(parent_oid) => format!("-p {parent_oid}"),
                        OidOrLabel::Label(parent_label) => {
                            format!("-p refs/rewritten/{parent_label}")
                        }
                    })
                    .join(" ");
                // FIXME: Untested, this probably doesn't work. Currently,
                // on-disk rebases for merges with replacement commits are not
                // generated by any command, so this code path is never
                // triggered.
                format!("exec git show -s --format=%B {replacement_commit_oid} | git commit-tree {parents} {replacement_commit_oid}^{{tree}}")
            }
            RebaseCommand::Break => "break".to_string(),
            RebaseCommand::RegisterExtraPostRewriteHook => {
                "exec git branchless hook-register-extra-post-rewrite-hook".to_string()
            }
            RebaseCommand::DetectEmptyCommit { commit_oid } => {
                format!("exec git branchless hook-detect-empty-commit {commit_oid}")
            }
            RebaseCommand::SkipUpstreamAppliedCommit { commit_oid } => {
                format!("exec git branchless hook-skip-upstream-applied-commit {commit_oid}")
            }
        }
    }
}

/// Represents a sequence of commands that can be executed to carry out a rebase
/// operation.
#[derive(Debug)]
pub struct RebasePlan {
    /// The first commit OID that will be checked out. This is necessary to
    /// support on-disk rebases.
    pub first_dest_oid: NonZeroOid,

    /// The commands to run.
    pub commands: Vec<RebaseCommand>,
}

/// A token representing that the rebase plan has been checked for validity.
#[derive(Clone, Debug)]
pub struct RebasePlanPermissions {
    pub(crate) build_options: BuildRebasePlanOptions,
    pub(crate) allowed_commits: CommitSet,
}

impl RebasePlanPermissions {
    /// Construct a new `RebasePlanPermissions`.
    pub fn verify_rewrite_set(
        dag: &Dag,
        build_options: BuildRebasePlanOptions,
        commits: &CommitSet,
    ) -> eyre::Result<Result<Self, BuildRebasePlanError>> {
        // This isn't necessary for correctness, but helps to produce a better
        // error message which indicates the magnitude of the issue.
        let commits = dag.query_descendants(commits.clone())?;

        let public_commits = dag.query_public_commits_slow()?;
        if !build_options.force_rewrite_public_commits {
            let public_commits_to_move = public_commits.intersection(&commits);
            if !dag.set_is_empty(&public_commits_to_move)? {
                return Ok(Err(BuildRebasePlanError::MovePublicCommits {
                    public_commits_to_move,
                }));
            }
        }

        Ok(Ok(RebasePlanPermissions {
            build_options,
            allowed_commits: commits,
        }))
    }
}

/// Represents the commits (as OIDs) that will be used to build a rebase plan.
#[derive(Clone, Debug)]
struct ConstraintGraph<'a> {
    dag: &'a Dag,
    permissions: &'a RebasePlanPermissions,

    /// This is a mapping from `x` to `y` if `x` must be applied before `y`
    inner: HashMap<NonZeroOid, HashSet<NonZeroOid>>,

    /// A mapping of commits being fixed up to the commits being absorbed into them.
    fixups: HashMap<NonZeroOid, HashSet<NonZeroOid>>,
}

impl<'a> ConstraintGraph<'a> {
    pub fn new(dag: &'a Dag, permissions: &'a RebasePlanPermissions) -> Self {
        Self {
            dag,
            permissions,
            inner: HashMap::new(),
            fixups: HashMap::new(),
        }
    }

    pub fn add_constraints(&mut self, constraints: &Vec<Constraint>) -> eyre::Result<()> {
        for constraint in constraints {
            match constraint {
                Constraint::MoveChildren { .. } => {
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

                Constraint::FixUpCommit {
                    commit_to_fixup_oid,
                    fixup_commit_oid,
                } => {
                    // remove previous (if any) constraints on commit
                    for commits in self.inner.values_mut() {
                        commits.remove(fixup_commit_oid);
                    }

                    self.fixups
                        .entry(*commit_to_fixup_oid)
                        .or_default()
                        .insert(*fixup_commit_oid);
                }
            }
        }

        let range_heads: HashSet<&NonZeroOid> = constraints
            .iter()
            .filter_map(|c| match c {
                Constraint::MoveSubtree { .. } => None,
                Constraint::MoveChildren {
                    parent_of_oid: _,
                    children_of_oid,
                }
                | Constraint::FixUpCommit {
                    commit_to_fixup_oid: _,
                    fixup_commit_oid: children_of_oid,
                } => Some(children_of_oid),
            })
            .collect();

        let mut move_children =
            |parent_of_oid: &NonZeroOid, children_of_oid: &NonZeroOid| -> eyre::Result<()> {
                let mut parent_oid = self.dag.get_only_parent_oid(*parent_of_oid)?;
                let commits_to_move = self.commits_to_move();

                // If parent_oid is part of another range and is itself
                // constrained, keep looking for an unconstrained ancestor.
                while range_heads.contains(&parent_oid) && commits_to_move.contains(&parent_oid) {
                    parent_oid = self.dag.get_only_parent_oid(parent_oid)?;
                }

                let commits_to_move: CommitSet = commits_to_move.into_iter().collect();
                let source_children: CommitSet = self
                    .dag
                    .query_children(CommitSet::from(*children_of_oid))?
                    .difference(&commits_to_move);
                let source_children = self.dag.filter_visible_commits(source_children)?;

                for child_oid in self.dag.commit_set_to_vec(&source_children)? {
                    self.inner.entry(parent_oid).or_default().insert(child_oid);
                }

                Ok(())
            };

        for constraint in constraints {
            match constraint {
                Constraint::MoveChildren {
                    parent_of_oid,
                    children_of_oid,
                } => move_children(parent_of_oid, children_of_oid)?,

                Constraint::FixUpCommit {
                    commit_to_fixup_oid: _,
                    fixup_commit_oid,
                } => move_children(fixup_commit_oid, fixup_commit_oid)?,

                Constraint::MoveSubtree { .. } => {
                    // do nothing; these were handled in the first pass
                }
            }
        }

        Ok(())
    }

    /// Add additional edges to the constraint graph for each descendant commit
    /// of a referred-to commit. This adds enough information to the constraint
    /// graph that it now represents the actual end-state commit graph that we
    /// want to create, not just a list of constraints.
    fn add_descendant_constraints(&mut self, effects: &Effects) -> eyre::Result<()> {
        let (effects, _progress) = effects.start_operation(OperationType::ConstrainCommits);
        let _effects = effects;

        let all_descendants_of_constrained_nodes = {
            let mut acc = Vec::new();
            let commits_to_fixup = self.commits_to_fixup();
            let commits_to_move: CommitSet = self
                .commits_to_move()
                .union(&commits_to_fixup)
                .cloned()
                .collect();
            let descendants = self.dag.query_descendants(commits_to_move.clone())?;
            let descendants = descendants.difference(&commits_to_move);
            let descendants = self.dag.filter_visible_commits(descendants)?;
            let descendant_oids = self.dag.commit_set_to_vec(&descendants)?;
            for descendant_oid in descendant_oids {
                let parents = self.dag.query_parent_names(descendant_oid)?;
                let parent_oids = parents
                    .into_iter()
                    .map(NonZeroOid::try_from)
                    .try_collect()?;
                acc.push(Constraint::MoveSubtree {
                    parent_oids,
                    child_oid: descendant_oid,
                });
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
        path.pop();
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
        if !self.dag.set_is_empty(&illegal_commits_to_move)? {
            Ok(Err(BuildRebasePlanError::MoveIllegalCommits {
                illegal_commits_to_move,
            }))
        } else {
            Ok(Ok(()))
        }
    }

    fn find_roots(&self) -> Vec<Constraint> {
        let unconstrained_fixup_nodes = &self.commits_to_fixup() - &self.commits_to_move();
        let unconstrained_nodes = {
            let unconstrained_nodes: HashSet<NonZeroOid> = self.parents().into_iter().collect();
            let unconstrained_nodes = &unconstrained_nodes - &self.commits_to_move();
            &unconstrained_nodes - &unconstrained_fixup_nodes
        };

        let root_edge_iter = unconstrained_nodes
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
            .flatten();

        let fixup_edge_iter = unconstrained_fixup_nodes
            .into_iter()
            .map(|commit_to_fixup_oid| Constraint::FixUpCommit {
                commit_to_fixup_oid,
                // HACK but this is unused
                fixup_commit_oid: commit_to_fixup_oid,
            });

        let mut root_edges: Vec<Constraint> = root_edge_iter.chain(fixup_edge_iter).collect();
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

    /// All of the commits being fixed up.
    pub fn commits_to_fixup(&self) -> HashSet<NonZeroOid> {
        self.fixups.keys().copied().collect()
    }

    /// All of the fixup commits. This is set of all commits which need
    /// to be absorbed into other commits and removed from the commit graph.
    pub fn fixup_commits(&self) -> HashSet<NonZeroOid> {
        self.fixups.values().flatten().copied().collect()
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
    ///
    /// When replacing commits and specifying a parent which is also being
    /// rebased, we need to refer to it via label instead of OID, to ensure that
    /// we use the rewritten parent.
    parent_labels: HashMap<NonZeroOid, String>,
}

/// Builder for a rebase plan. Unlike regular Git rebases, a `git-branchless`
/// rebase plan can move multiple unrelated subtrees to unrelated destinations.
#[derive(Clone, Debug)]
pub struct RebasePlanBuilder<'a> {
    dag: &'a Dag,
    permissions: RebasePlanPermissions,

    /// The constraints specified by the caller.
    initial_constraints: Vec<Constraint>,

    /// Mapping of commits that should be replaced to the commits that they should be replaced
    /// with.
    replacement_commits: HashMap<NonZeroOid, NonZeroOid>,

    /// Cache mapping from commit OID to the paths changed in the diff for that
    /// commit. The value is `None` if the commit doesn't have an associated
    /// diff (i.e. is a merge commit).
    pub(crate) touched_paths_cache: Arc<CHashMap<NonZeroOid, HashSet<PathBuf>>>,
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

    /// Indicates that `fixup_commit` should be squashed into `commit_to_fixup`
    /// and all of its descendants should be moved on top of its parent.
    FixUpCommit {
        commit_to_fixup_oid: NonZeroOid,
        fixup_commit_oid: NonZeroOid,
    },
}

/// Options used to build a rebase plan.
#[derive(Clone, Debug)]
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
    pub fn describe(&self, effects: &Effects, repo: &Repo, dag: &Dag) -> eyre::Result<()> {
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
                        glyphs.render(repo.friendly_describe_commit_from_oid(glyphs, *oid)?,)?,
                    )?;
                }
            }

            BuildRebasePlanError::MovePublicCommits {
                public_commits_to_move,
            } => {
                let example_bad_commit_oid =
                    dag.set_first(public_commits_to_move)?.ok_or_else(|| {
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
                        amount: dag.set_count(public_commits_to_move)?,
                        unit: ("public commit", "public commits")
                    },
                    effects
                        .get_glyphs()
                        .render(example_bad_commit.friendly_describe(effects.get_glyphs())?)?,
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
                    dag.commit_set_to_vec(illegal_commits_to_move)?
                )?;
            }
        }
        Ok(())
    }
}

impl<'a> RebasePlanBuilder<'a> {
    /// Constructor.
    pub fn new(dag: &'a Dag, permissions: RebasePlanPermissions) -> Self {
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
                                .parent_labels
                                .get(&parent_oid)
                                .map(|parent_label| OidOrLabel::Label(parent_label.clone()))
                        } else {
                            // This parent commit was not supposed to be
                            // rebased, so its OID won't change and we can
                            // address it OID directly.
                            Some(OidOrLabel::Oid(parent_oid))
                        }
                    })
                    .collect();

                let commits_to_merge = match commits_to_merge {
                    Some(commits_to_merge) => {
                        // We've already checked to make sure that all parent
                        // commits which are being moved have had their labels
                        // applied. However, if the commit has multiple parents
                        // which were *not* moved, then we would visit this
                        // commit and attempt to reapply it multiple times. The
                        // simplest fix is to not reapply merge commits which
                        // have already been applied.
                        //
                        // FIXME: O(n^2) algorithm.
                        let commit_already_applied = acc.iter().any(|command| match command {
                            RebaseCommand::Merge {
                                commit_oid,
                                commits_to_merge: _,
                            }
                            | RebaseCommand::Replace {
                                commit_oid,
                                replacement_commit_oid: _,
                                parents: _,
                            } => *commit_oid == current_commit.get_oid(),
                            _ => false,
                        });
                        if !commit_already_applied {
                            Some(commits_to_merge)
                        } else {
                            None
                        }
                    }

                    None => {
                        // Wait for the caller to come back to this commit later
                        // and then proceed to any child commits.
                        None
                    }
                };

                let commits_to_merge = match commits_to_merge {
                    None => {
                        // Some parents need to be committed. Wait for caller to come back to this commit later and then proceed to any child commits.
                        return Ok(acc);
                    }
                    Some(commits_to_merge) => {
                        // All parents have been committed.
                        commits_to_merge
                    }
                };

                let (first_parent, merge_parents) = match commits_to_merge.as_slice() {
                    [] => {
                        unreachable!("Already verified that there's at least one parent commit")
                    }
                    [first, rest @ ..] => (first, rest.to_vec()),
                };
                acc.push(RebaseCommand::Reset {
                    target: first_parent.clone(),
                });
                acc.push(
                    match self.replacement_commits.get(&current_commit.get_oid()) {
                        None => RebaseCommand::Merge {
                            commit_oid: current_commit.get_oid(),
                            commits_to_merge: merge_parents.to_vec(),
                        },
                        Some(replacement_commit_oid) => RebaseCommand::Replace {
                            commit_oid: current_commit.get_oid(),
                            replacement_commit_oid: *replacement_commit_oid,
                            parents: commits_to_merge,
                        },
                    },
                );
            } else if state
                .constraints
                .fixup_commits()
                .contains(&current_commit.get_oid())
            {
                // Do nothing for fixup commits.
            } else {
                // Normal one-parent commit (or a zero-parent commit?), just
                // rebase it and continue.
                let original_commit_oid = current_commit.get_oid();
                match self.replacement_commits.get(&original_commit_oid) {
                    Some(replacement_commit_oid) => {
                        let replacement_commit =
                            repo.find_commit_or_fail(*replacement_commit_oid)?;
                        let new_parents = replacement_commit
                            .get_parent_oids()
                            .into_iter()
                            .map(|parent_oid| match state.parent_labels.get(&parent_oid) {
                                None => OidOrLabel::Oid(parent_oid),
                                Some(parent_label) => OidOrLabel::Label(parent_label.clone()),
                            })
                            .collect_vec();
                        acc.push(RebaseCommand::Replace {
                            commit_oid: original_commit_oid,
                            replacement_commit_oid: *replacement_commit_oid,
                            parents: new_parents,
                        });
                    }
                    None => {
                        let commits_to_apply_oids = match state
                            .constraints
                            .fixups
                            .get(&original_commit_oid)
                        {
                            None => vec![original_commit_oid],
                            Some(fixup_commit_oids) => {
                                let mut commits_to_apply = vec![original_commit_oid];
                                commits_to_apply.extend(fixup_commit_oids);

                                let commit_set: CommitSet = commits_to_apply.into_iter().collect();
                                let commits_to_apply =
                                    sorted_commit_set(repo, state.constraints.dag, &commit_set)?;

                                commits_to_apply
                                    .iter()
                                    .map(|commit| commit.get_oid())
                                    .collect()
                            }
                        };
                        acc.push(RebaseCommand::Pick {
                            original_commit_oid,
                            commits_to_apply_oids,
                        });
                        acc.push(RebaseCommand::DetectEmptyCommit {
                            commit_oid: current_commit.get_oid(),
                        });
                    }
                };
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
            if child_commits.iter().any(|child_commit| {
                child_commit.get_parent_count() > 1
                    || self
                        .replacement_commits
                        .contains_key(&child_commit.get_oid())
            }) {
                // If this commit has any merge commits or replaced commits as
                // children, create a label so that the child can reference this
                // commit later for merging.
                let command_num = acc.len();
                let label_name = self.make_label_name(state, format!("parent-{command_num}"));
                state
                    .parent_labels
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
                only_child_commit,
                upstream_patch_ids,
                acc,
            )?;
            Ok(acc)
        } else {
            let command_num = acc.len();
            let label_name = self.make_label_name(state, format!("label-{command_num}"));
            let mut acc = acc;
            acc.push(RebaseCommand::CreateLabel {
                label_name: label_name.clone(),
            });
            for child_commit in child_commits {
                acc = self.make_rebase_plan_for_current_commit(
                    effects,
                    repo,
                    state,
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

    /// Generate a sequence of rebase steps that cause the commit at
    /// `source_oid` to be squashed into `dest_oid`, and for the descendants
    /// of `source_oid` to be rebased on top of its parent.
    pub fn fixup_commit(
        &mut self,
        source_oid: NonZeroOid,
        dest_oid: NonZeroOid,
    ) -> eyre::Result<()> {
        self.initial_constraints.push(Constraint::FixUpCommit {
            commit_to_fixup_oid: dest_oid,
            fixup_commit_oid: source_oid,
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
            parent_labels: Default::default(),
        };

        let (effects, _progress) = effects.start_operation(OperationType::BuildRebasePlan);

        let BuildRebasePlanOptions {
            force_rewrite_public_commits: _,
            dump_rebase_constraints,
            dump_rebase_plan,
            detect_duplicate_commits_via_patch_id,
        } = &self.permissions.build_options;
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
            // NOTE that these are coming from find_roots() and are not the
            // original Constraints used to build the plan
            let (parent_oids, child_oid) = match constraint {
                Constraint::MoveSubtree {
                    parent_oids,
                    child_oid,
                } => (parent_oids, child_oid),

                Constraint::FixUpCommit {
                    commit_to_fixup_oid,
                    fixup_commit_oid: _,
                } => {
                    let parents = repo.find_commit_or_fail(commit_to_fixup_oid)?.get_parent_oids();
                    (parents, commit_to_fixup_oid)
                },

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
            println!("Rebase plan: {rebase_plan:#?}");
        }
        Ok(Ok(rebase_plan))
    }

    fn check_all_commits_included_in_rebase_plan(
        state: &BuildState,
        rebase_commands: &[RebaseCommand],
    ) {
        let included_commit_oids: HashSet<NonZeroOid> = rebase_commands
            .iter()
            .flat_map(|rebase_command| match rebase_command {
                RebaseCommand::CreateLabel { label_name: _ }
                | RebaseCommand::Reset { target: _ }
                | RebaseCommand::Break
                | RebaseCommand::RegisterExtraPostRewriteHook
                | RebaseCommand::DetectEmptyCommit { commit_oid: _ } => Vec::new(),
                RebaseCommand::Pick {
                    original_commit_oid,
                    commits_to_apply_oids,
                } => {
                    let mut commit_oids = vec![*original_commit_oid];
                    commit_oids.extend(commits_to_apply_oids);
                    commit_oids
                }
                RebaseCommand::Merge {
                    commit_oid,
                    commits_to_merge: _,
                }
                | RebaseCommand::Replace {
                    commit_oid,
                    replacement_commit_oid: _,
                    parents: _,
                }
                | RebaseCommand::SkipUpstreamAppliedCommit { commit_oid } => vec![*commit_oid],
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
                self.dag.query_gca_all(commit_set)
            })
            .try_collect()?;
        let merge_base_oids = union_all(&merge_base_oids);

        let touched_commit_oids: Vec<NonZeroOid> =
            state.constraints.commits_to_move().into_iter().collect();

        let path = {
            let (effects, _progress) = effects.start_operation(OperationType::WalkCommits);
            let _effects = effects;
            self.dag
                .query_range(merge_base_oids, dest_oids.iter().copied().collect())
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
        let path = self.dag.commit_set_to_vec(&path)?;

        let touched_commits: Vec<Commit> = touched_commit_oids
            .into_iter()
            .map(|oid| repo.find_commit(oid))
            .flatten_ok()
            .try_collect()?;
        let local_touched_paths: Vec<HashSet<PathBuf>> = touched_commits
            .into_iter()
            .map(|commit| repo.get_paths_touched_by_commit(&commit))
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
                if touched_paths_cache.is_empty() {
                    // Fast path for when the cache hasn't been populated.
                    path.into_iter()
                        .with_progress(progress)
                        .map(CacheLookupResult::NotCached)
                        .collect()
                } else {
                    path.into_iter()
                        .with_progress(progress)
                        .map(|commit_oid| match touched_paths_cache.get(&commit_oid) {
                            Some(upstream_touched_paths) => {
                                if Self::should_check_patch_id(
                                    &upstream_touched_paths,
                                    &local_touched_paths,
                                ) {
                                    CacheLookupResult::Cached(Some(commit_oid))
                                } else {
                                    CacheLookupResult::Cached(None)
                                }
                            }
                            None => CacheLookupResult::NotCached(commit_oid),
                        })
                        .collect()
                }
            };

            let (effects, progress) = effects.start_operation(OperationType::FilterByTouchedPaths);
            let _effects = effects;
            pool.install(|| {
                progress.notify_progress(0, path.len());
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
        upstream_touched_paths: &HashSet<PathBuf>,
        local_touched_paths: &[HashSet<PathBuf>],
    ) -> bool {
        // It's possible that the same commit was applied after a parent
        // commit renamed a certain path. In that case, this check won't
        // trigger. We'll rely on the empty-commit check after the
        // commit has been made to deduplicate the commit in that case.
        // FIXME: this code path could be optimized further.
        local_touched_paths.iter().contains(upstream_touched_paths)
    }
}
