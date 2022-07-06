//! Move commits and subtrees from one place to another.
//!
//! Under the hood, this makes use of Git's advanced rebase functionality, which
//! is also used to preserve merge commits using the `--rebase-merges` option.

use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::Write;
use std::time::SystemTime;

use eden_dag::DagAlgorithm;
use lib::core::repo_ext::RepoExt;
use lib::util::ExitCode;
use rayon::ThreadPoolBuilder;
use tracing::instrument;

use crate::opts::{MoveOptions, Revset};
use crate::revset::resolve_commits;
use lib::core::config::get_restack_preserve_timestamps;
use lib::core::dag::{commit_set_to_vec_unsorted, union_all, CommitSet, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanOptions, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    MergeConflictRemediation, RebasePlanBuilder, RepoResource,
};
use lib::git::{GitRunInfo, NonZeroOid, Repo};

#[instrument]
fn resolve_base_commit(
    dag: &Dag,
    merge_base_oid: Option<NonZeroOid>,
    oid: NonZeroOid,
) -> eyre::Result<NonZeroOid> {
    let bases = match merge_base_oid {
        Some(merge_base_oid) => {
            let range = dag
                .query()
                .range(CommitSet::from(merge_base_oid), CommitSet::from(oid))?;
            let roots = dag.query().roots(range.clone())?;
            dag.query().children(roots)?.intersection(&range)
        }
        None => {
            let ancestors = dag.query().ancestors(CommitSet::from(oid))?;
            dag.query().roots(ancestors)?
        }
    };

    match bases.first()? {
        Some(base) => NonZeroOid::try_from(base),
        None => Ok(oid),
    }
}

/// Move a subtree from one place to another.
#[instrument]
pub fn r#move(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    sources: Vec<Revset>,
    dest: Option<Revset>,
    bases: Vec<Revset>,
    ranges: Vec<Revset>,
    exacts: Vec<Revset>,
    insert: bool,
    move_options: &MoveOptions,
) -> eyre::Result<ExitCode> {
    let repo = Repo::from_current_dir()?;
    let head_oid = repo.get_head_info()?.oid;

    let dest = match dest {
        Some(dest) => dest,
        None => match head_oid {
            Some(oid) => Revset(oid.to_string()),
            None => {
                writeln!(effects.get_output_stream(), "No --dest argument was provided, and no OID for HEAD is available as a default")?;
                return Ok(ExitCode(1));
            }
        },
    };

    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let source_oids: CommitSet = match resolve_commits(effects, &repo, &mut dag, sources) {
        Ok(commit_sets) => union_all(&commit_sets),
        Err(err) => {
            err.describe(effects)?;
            return Ok(ExitCode(1));
        }
    };
    let base_oids: CommitSet = match resolve_commits(effects, &repo, &mut dag, bases) {
        Ok(commit_sets) => union_all(&commit_sets),
        Err(err) => {
            err.describe(effects)?;
            return Ok(ExitCode(1));
        }
    };
    let range_oids = {
        let mut range_oids: HashMap<NonZeroOid, NonZeroOid> = HashMap::new();
        for range in ranges.iter() {
            let commit_set = match resolve_commits(effects, &repo, &mut dag, vec![range.clone()]) {
                Ok(commit_sets) => union_all(&commit_sets),
                Err(err) => {
                    err.describe(effects)?;
                    return Ok(ExitCode(1));
                }
            };

            let source_roots: CommitSet = dag.query().roots(commit_set.clone())?;
            let source_root = match commit_set_to_vec_unsorted(&source_roots)?.as_slice() {
                [oid] => *oid,
                other => {
                    let Revset(expr) = range;
                    writeln!(
                        effects.get_error_stream(),
                        // FIXME wrap this @ 80 chars
                        "The --range flag can only be used to move ranges with exactly 1 root (got {}): {}",
                        other.len(),
                        expr
                    )?;
                    return Ok(ExitCode(1));
                }
            };
            let source_heads: CommitSet = dag.query().heads(commit_set)?;
            let source_head = match commit_set_to_vec_unsorted(&source_heads)?.as_slice() {
                [oid] => *oid,
                other => {
                    let Revset(expr) = range;
                    writeln!(
                    effects.get_error_stream(),
                        // FIXME wrap this @ 80 chars
                        "The --range flag can only be used to move ranges with exactly 1 head (got {}): {}",
                        other.len(),
                        expr
                    )?;
                    return Ok(ExitCode(1));
                }
            };
            range_oids.insert(source_root, source_head);
        }

        let exact_oids: CommitSet = match resolve_commits(effects, &repo, &mut dag, exacts) {
            Ok(commit_sets) => union_all(&commit_sets),
            Err(err) => {
                err.describe(effects)?;
                return Ok(ExitCode(1));
            }
        };
        for exact_oid in commit_set_to_vec_unsorted(&exact_oids)? {
            range_oids.insert(exact_oid, exact_oid);
        }

        range_oids
    };

    let dest_oid: NonZeroOid = match resolve_commits(effects, &repo, &mut dag, vec![dest.clone()]) {
        Ok(commit_sets) => match commit_set_to_vec_unsorted(&commit_sets[0])?.as_slice() {
            [only_commit_oid] => *only_commit_oid,
            other => {
                let Revset(expr) = dest;
                writeln!(
                    effects.get_error_stream(),
                    "Expected revset to expand to exactly 1 commit (got {}): {}",
                    other.len(),
                    expr,
                )?;
                return Ok(ExitCode(1));
            }
        },
        Err(err) => {
            err.describe(effects)?;
            return Ok(ExitCode(1));
        }
    };

    let base_oids = {
        let mut result = Vec::new();
        for base_oid in commit_set_to_vec_unsorted(&base_oids)? {
            let merge_base_oid = dag.get_one_merge_base_oid(effects, &repo, base_oid, dest_oid)?;
            let base_commit_oid = resolve_base_commit(&dag, merge_base_oid, base_oid)?;
            result.push(CommitSet::from(base_commit_oid))
        }
        union_all(&result)
    };
    let source_oids = source_oids.union(&base_oids);

    let source_oids = if source_oids.is_empty()? && range_oids.is_empty() {
        match head_oid {
            Some(head_oid) => CommitSet::from(head_oid),
            None => {
                writeln!(effects.get_output_stream(), "No --source, --base or --range arguments were provided, and no OID for HEAD is available as a default")?;
                return Ok(ExitCode(1));
            }
        }
    } else {
        source_oids
    };

    let MoveOptions {
        force_in_memory,
        force_on_disk,
        detect_duplicate_commits_via_patch_id,
        resolve_merge_conflicts,
        dump_rebase_constraints,
        dump_rebase_plan,
    } = *move_options;
    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "move")?;
    let pool = ThreadPoolBuilder::new().build()?;
    let repo_pool = RepoResource::new_pool(&repo)?;
    let rebase_plan = {
        let mut builder = RebasePlanBuilder::new(&dag);

        let source_roots = dag.query().roots(source_oids.clone())?;
        for source_root in commit_set_to_vec_unsorted(&source_roots)? {
            builder.move_subtree(source_root, dest_oid)?;
        }

        for (range_root, range_head) in range_oids.iter() {
            let source_parent = match dag.get_only_parent_oid(*range_root) {
                Ok(oid) => oid,
                Err(_) => {
                    writeln!(
                        effects.get_output_stream(),
                        "The --range and --exact flag can only be used to move ranges or commits with exactly 1 parent.",
                    )?;
                    return Ok(ExitCode(1));
                }
            };

            let range_children: CommitSet = dag
                .query()
                .children(CommitSet::from(*range_head))?
                .difference(&dag.obsolete_commits);

            for range_child in commit_set_to_vec_unsorted(&range_children)? {
                // If the range being extracted has any child commits, then we
                // need to move each of those subtrees up to the parent commit
                // of the range. If, however, we're inserting the range and the
                // destination commit is in one of those subtrees, then we
                // should only move the commits from the root of that child
                // subtree up to (and including) the destination commmit.
                if insert
                    && dag
                        .query()
                        .is_ancestor(range_child.into(), dest_oid.into())?
                {
                    builder.move_range(range_child, dest_oid, source_parent)?;
                } else {
                    builder.move_subtree(range_child, source_parent)?;
                }
            }

            builder.move_range(*range_root, *range_head, dest_oid)?;
        }

        if insert {
            let source_head = {
                let range_heads: CommitSet = range_oids.values().cloned().into_iter().collect();
                let source_heads: CommitSet = dag
                    .query()
                    .heads(dag.query().descendants(source_oids)?)?
                    .union(&range_heads);
                match commit_set_to_vec_unsorted(&source_heads)?.as_slice() {
                    [oid] => *oid,
                    _ => {
                        writeln!(
                            effects.get_output_stream(),
                            "The --insert flag cannot be used when moving subtrees or ranges with multiple heads."
                        )?;
                        return Ok(ExitCode(1));
                    }
                }
            };

            let dest_children: CommitSet = dag
                .query()
                .children(CommitSet::from(dest_oid))?
                .difference(&dag.obsolete_commits);

            let range_roots: CommitSet = range_oids.keys().cloned().into_iter().collect();
            let source_roots = source_roots.union(&range_roots);
            for dest_child in commit_set_to_vec_unsorted(&dest_children)? {
                for source_root in commit_set_to_vec_unsorted(&source_roots)? {
                    if dag
                        .query()
                        .is_ancestor(dest_child.into(), source_root.into())?
                    {
                        // If this child subtree actually contains the source
                        // subtree being moved, then we should only move the commit
                        // range *up to* the source subtree, not the entire child
                        // subtree.
                        let source_parent = match dag.get_only_parent_oid(source_root) {
                            Ok(oid) => oid,
                            Err(_) => {
                                writeln!(
                                    effects.get_output_stream(),
                                    "The --insert flag can only be used when moving subtrees with exactly 1 parent."
                                )?;
                                return Ok(ExitCode(1));
                            }
                        };
                        builder.move_range(dest_child, source_parent, source_head)?;
                    } else {
                        builder.move_subtree(dest_child, source_head)?;
                    }
                }
            }
        }
        builder.build(
            effects,
            &pool,
            &repo_pool,
            &BuildRebasePlanOptions {
                dump_rebase_constraints,
                dump_rebase_plan,
                detect_duplicate_commits_via_patch_id,
            },
        )?
    };
    let result = match rebase_plan {
        Ok(None) => {
            writeln!(effects.get_output_stream(), "Nothing to do.")?;
            return Ok(ExitCode(0));
        }
        Ok(Some(rebase_plan)) => {
            let options = ExecuteRebasePlanOptions {
                now,
                event_tx_id,
                preserve_timestamps: get_restack_preserve_timestamps(&repo)?,
                force_in_memory,
                force_on_disk,
                resolve_merge_conflicts,
                check_out_commit_options: Default::default(),
            };
            execute_rebase_plan(
                effects,
                git_run_info,
                &repo,
                &event_log_db,
                &rebase_plan,
                &options,
            )?
        }
        Err(err) => {
            err.describe(effects, &repo)?;
            return Ok(ExitCode(1));
        }
    };

    match result {
        ExecuteRebasePlanResult::Succeeded { rewritten_oids: _ } => Ok(ExitCode(0)),

        ExecuteRebasePlanResult::DeclinedToMerge { merge_conflict } => {
            merge_conflict.describe(effects, &repo, MergeConflictRemediation::Retry)?;
            Ok(ExitCode(1))
        }

        ExecuteRebasePlanResult::Failed { exit_code } => Ok(exit_code),
    }
}
