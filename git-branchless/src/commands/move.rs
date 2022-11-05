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

use crate::opts::{MoveOptions, ResolveRevsetOptions, Revset};
use crate::revset::resolve_commits;
use lib::core::config::{
    get_hint_enabled, get_hint_string, get_restack_preserve_timestamps,
    print_hint_suppression_notice, Hint,
};
use lib::core::dag::{commit_set_to_vec, sorted_commit_set, union_all, CommitSet, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::rewrite::{
    execute_rebase_plan, BuildRebasePlanOptions, ExecuteRebasePlanOptions, ExecuteRebasePlanResult,
    MergeConflictRemediation, RebasePlanBuilder, RebasePlanPermissions, RepoResource,
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
    exacts: Vec<Revset>,
    resolve_revset_options: &ResolveRevsetOptions,
    move_options: &MoveOptions,
    insert: bool,
) -> eyre::Result<ExitCode> {
    let sources_provided = !sources.is_empty();
    let bases_provided = !bases.is_empty();
    let exacts_provided = !exacts.is_empty();
    let dest_provided = dest.is_some();
    let should_sources_default_to_head = !sources_provided && !bases_provided && !exacts_provided;

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

    let source_oids: CommitSet =
        match resolve_commits(effects, &repo, &mut dag, &sources, resolve_revset_options) {
            Ok(commit_sets) => union_all(&commit_sets),
            Err(err) => {
                err.describe(effects)?;
                return Ok(ExitCode(1));
            }
        };
    let base_oids: CommitSet =
        match resolve_commits(effects, &repo, &mut dag, &bases, resolve_revset_options) {
            Ok(commit_sets) => union_all(&commit_sets),
            Err(err) => {
                err.describe(effects)?;
                return Ok(ExitCode(1));
            }
        };
    let exact_components = match resolve_commits(
        effects,
        &repo,
        &mut dag,
        &exacts,
        resolve_revset_options,
    ) {
        Ok(commit_sets) => {
            let exact_oids = union_all(&commit_sets);
            let mut components: HashMap<NonZeroOid, CommitSet> = HashMap::new();

            for component in dag.get_connected_components(&exact_oids)?.into_iter() {
                let component_roots = dag.query().roots(component.clone())?;
                let component_root = match commit_set_to_vec(&component_roots)?.as_slice() {
                    [only_commit_oid] => *only_commit_oid,
                    _ => {
                        writeln!(
                            effects.get_error_stream(),
                            "The --exact flag can only be used to move ranges with exactly 1 root.\n\
                             Received range with {} roots: {:?}",
                            component_roots.count()?,
                            component_roots
                        )?;
                        return Ok(ExitCode(1));
                    }
                };

                let component_parents = dag.query().parents(CommitSet::from(component_root))?;
                if component_parents.count()? != 1 {
                    writeln!(
                        effects.get_output_stream(),
                        "The --exact flag can only be used to move ranges or commits with exactly 1 parent.\n\
                         Received range with {} parents: {:?}",
                        component_parents.count()?,
                        component_parents
                    )?;
                    return Ok(ExitCode(1));
                };

                components.insert(component_root, component);
            }

            components
        }
        Err(err) => {
            err.describe(effects)?;
            return Ok(ExitCode(1));
        }
    };

    let dest_oid: NonZeroOid = match resolve_commits(
        effects,
        &repo,
        &mut dag,
        &[dest.clone()],
        resolve_revset_options,
    ) {
        Ok(commit_sets) => match commit_set_to_vec(&commit_sets[0])?.as_slice() {
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

    let base_oids = if should_sources_default_to_head {
        match head_oid {
            Some(head_oid) => CommitSet::from(head_oid),
            None => {
                writeln!(effects.get_output_stream(), "No --source or --base arguments were provided, and no OID for HEAD is available as a default")?;
                return Ok(ExitCode(1));
            }
        }
    } else {
        base_oids
    };
    let base_oids = {
        let mut result = Vec::new();
        for base_oid in commit_set_to_vec(&base_oids)? {
            let merge_base_oid = dag.get_one_merge_base_oid(effects, &repo, base_oid, dest_oid)?;
            let base_commit_oid = resolve_base_commit(&dag, merge_base_oid, base_oid)?;
            result.push(CommitSet::from(base_commit_oid))
        }
        union_all(&result)
    };
    let source_oids = source_oids.union(&base_oids);

    if let Some(head_oid) = head_oid {
        if get_hint_enabled(&repo, Hint::MoveImplicitHeadArgument)? {
            let should_warn_base = !sources_provided
                && bases_provided
                && base_oids.contains(&head_oid.into())?
                && base_oids.count()? == 1;
            if should_warn_base {
                writeln!(
                    effects.get_output_stream(),
                    "{}: you can omit the --base flag in this case, as it defaults to HEAD",
                    effects.get_glyphs().render(get_hint_string())?,
                )?;
            }

            let should_warn_dest = dest_provided && dest_oid == head_oid;
            if should_warn_dest {
                writeln!(
                    effects.get_output_stream(),
                    "{}: you can omit the --dest flag in this case, as it defaults to HEAD",
                    effects.get_glyphs().render(get_hint_string())?,
                )?;
            }

            if should_warn_base || should_warn_dest {
                print_hint_suppression_notice(effects, Hint::MoveImplicitHeadArgument)?;
            }
        }
    }
    drop(base_oids);

    let MoveOptions {
        force_rewrite_public_commits,
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
        let build_options = BuildRebasePlanOptions {
            force_rewrite_public_commits,
            dump_rebase_constraints,
            dump_rebase_plan,
            detect_duplicate_commits_via_patch_id,
        };
        let permissions = {
            let commits_to_move = source_oids.clone();
            let commits_to_move = commits_to_move.union(&union_all(
                &exact_components.values().cloned().collect::<Vec<_>>(),
            ));
            let commits_to_move = if insert {
                commits_to_move.union(&dag.query().children(CommitSet::from(dest_oid))?)
            } else {
                commits_to_move
            };

            match RebasePlanPermissions::verify_rewrite_set(&dag, &build_options, &commits_to_move)?
            {
                Ok(permissions) => permissions,
                Err(err) => {
                    err.describe(effects, &repo)?;
                    return Ok(ExitCode(1));
                }
            }
        };
        let mut builder = RebasePlanBuilder::new(&dag, permissions);

        let source_roots = dag.query().roots(source_oids.clone())?;
        for source_root in commit_set_to_vec(&source_roots)? {
            builder.move_subtree(source_root, vec![dest_oid])?;
        }

        let component_roots: CommitSet = exact_components.keys().cloned().collect();
        let component_roots: Vec<NonZeroOid> = sorted_commit_set(&repo, &dag, &component_roots)?
            .iter()
            .map(|commit| commit.get_oid())
            .collect();
        for component_root in component_roots.iter().cloned() {
            let component = exact_components.get(&component_root).unwrap();

            // Find the non-inclusive ancestor components of the current root
            let mut possible_destinations: Vec<NonZeroOid> = vec![];
            for root in component_roots.iter().cloned() {
                let component = exact_components.get(&root).unwrap();
                if !component.contains(&component_root.into())?
                    && dag
                        .query()
                        .is_ancestor(root.into(), component_root.into())?
                {
                    possible_destinations.push(root);
                }
            }

            let component_dest_oid = if possible_destinations.is_empty() {
                dest_oid
            } else {
                // If there was a merge commit somewhere outside of the selected
                // components, then it's possible that the current component
                // could have multiple possible parents.
                //
                // To check for this, we can confirm that the nearest
                // destination component is an ancestor of the previous (ie next
                // nearest). This works because possible_destinations is made
                // from component_roots, which has been sorted topologically; so
                // each included component should "come after" the previous
                // component.
                for i in 1..possible_destinations.len() {
                    if !dag.query().is_ancestor(
                        possible_destinations[i - 1].into(),
                        possible_destinations[i].into(),
                    )? {
                        writeln!(
                            effects.get_output_stream(),
                            "This operation cannot be completed because the {} at {}\n\
                              has multiple possible parents also being moved. Please retry this operation\n\
                              without this {}, or with only 1 possible parent.",
                            if component.count()? == 1 {
                                "commit"
                            } else {
                                "range of commits rooted"
                            },
                            component_root,
                            if component.count()? == 1 {
                                "commit"
                            } else {
                                "range of commits"
                            },
                        )?;
                        return Ok(ExitCode(1));
                    }
                }

                let nearest_component = exact_components
                    .get(&possible_destinations[possible_destinations.len() - 1])
                    .unwrap();
                // The current component could be descended from any commit
                // in nearest_component, not just it's head.
                let dest_ancestor = dag
                    .query()
                    .ancestors(CommitSet::from(component_root))?
                    .intersection(nearest_component);
                match dag.query().heads(dest_ancestor.clone())?.first()? {
                    Some(head) => NonZeroOid::try_from(head)?,
                    None => dest_oid,
                }
            };

            // Again, we've already confirmed that each component has but 1 parent
            let component_parent = NonZeroOid::try_from(
                dag.query()
                    .parents(CommitSet::from(component_root))?
                    .first()?
                    .unwrap(),
            )?;
            let component_children: CommitSet = dag
                .query()
                .children(component.clone())?
                .difference(component)
                .intersection(dag.query_visible_commits()?);

            for component_child in commit_set_to_vec(&component_children)? {
                // If the range being extracted has any child commits, then we
                // need to move each of those subtrees up to the parent commit
                // of the range. If, however, we're inserting the range and the
                // destination commit is in one of those subtrees, then we
                // should only move the commits from the root of that child
                // subtree up to (and including) the destination commmit.
                if insert
                    && dag
                        .query()
                        .is_ancestor(component_child.into(), component_dest_oid.into())?
                {
                    builder.move_range(component_child, component_dest_oid, component_parent)?;
                } else {
                    builder.move_subtree(component_child, vec![component_parent])?;
                }
            }

            builder.move_subtree(component_root, vec![component_dest_oid])?;
        }

        if insert {
            let source_head = {
                let exact_head = if component_roots.is_empty() {
                    CommitSet::empty()
                } else {
                    // As long as component_roots has been sorted topologically,
                    // we only need to compare adjacent elements to confirm a
                    // single lineage.
                    for i in 1..component_roots.len() {
                        if !dag
                            .query()
                            .is_ancestor(component_roots[i - 1].into(), component_roots[i].into())?
                        {
                            writeln!(
                                effects.get_output_stream(),
                                "The --insert and --exact flags can only be used together when moving commits or\n\
                                 ranges that form a single lineage, but {} is not an ancestor of {}.",
                                component_roots[i - 1],
                                component_roots[i]
                            )?;
                            return Ok(ExitCode(1));
                        }
                    }

                    let head_component = exact_components
                        .get(&component_roots[component_roots.len() - 1])
                        .unwrap()
                        .clone();
                    dag.query().heads(head_component)?
                };
                let source_heads: CommitSet = dag
                    .query()
                    .heads(dag.query().descendants(source_oids.clone())?)?
                    .union(&exact_head);
                match commit_set_to_vec(&source_heads)?.as_slice() {
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

            let exact_components = exact_components
                .values()
                .cloned()
                .collect::<Vec<CommitSet>>();
            let exact_oids = union_all(&exact_components);
            // Children of dest_oid that are not themselves being moved.
            let dest_children: CommitSet = dag
                .query()
                .children(CommitSet::from(dest_oid))?
                .difference(&source_oids)
                .difference(&exact_oids)
                .intersection(dag.query_visible_commits()?);

            for dest_child in commit_set_to_vec(&dest_children)? {
                builder.move_subtree(dest_child, vec![source_head])?;
            }
        }
        builder.build(effects, &pool, &repo_pool)?
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
