use bstr::ByteSlice;
use eden_dag::set::hints::Hints;

use lib::core::dag::CommitSet;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::repo_ext::RepoExt;
use lib::core::rewrite::find_rewrite_target;
use lib::git::{
    get_latest_test_command_path, get_test_tree_dir, CategorizedReferenceName, Commit,
    MaybeZeroOid, Repo, SerializedNonZeroOid, SerializedTestResult, TEST_ABORT_EXIT_CODE,
    TEST_INDETERMINATE_EXIT_CODE, TEST_SUCCESS_EXIT_CODE,
};
use std::borrow::Cow;
use std::collections::HashMap;
use tracing::{instrument, warn};

use eyre::Context as EyreContext;
use lazy_static::lazy_static;

use crate::eval::{
    eval0, eval0_or_1, eval0_or_1_pattern, eval1, eval1_pattern, eval2, eval_number_rhs, Context,
    EvalError, EvalResult,
};
use crate::pattern::{make_pattern_matcher_set, Pattern};
use crate::pattern::{PatternError, PatternMatcher};
use crate::Expr;

type FnType = &'static (dyn Fn(&mut Context, &str, &[Expr]) -> EvalResult + Sync);
lazy_static! {
    pub(super) static ref FUNCTIONS: HashMap<&'static str, FnType> = {
        let functions: &[(&'static str, FnType)] = &[
            ("all", &fn_all),
            ("none", &fn_none),
            ("union", &fn_union),
            ("intersection", &fn_intersection),
            ("difference", &fn_difference),
            ("only", &fn_only),
            ("range", &fn_range),
            ("not", &fn_not),
            ("ancestors", &fn_ancestors),
            ("ancestors.nth", &fn_nthancestor),
            ("descendants", &fn_descendants),
            ("parents", &fn_parents),
            ("parents.nth", &fn_parents_nth),
            ("children", &fn_children),
            ("siblings", &fn_siblings),
            ("roots", &fn_roots),
            ("heads", &fn_heads),
            ("branches", &fn_branches),
            ("main", &fn_main),
            ("public", &fn_public),
            ("draft", &fn_draft),
            ("stack", &fn_stack),
            ("message", &fn_message),
            ("paths.changed", &fn_path_changed),
            ("author.name", &fn_author_name),
            ("author.email", &fn_author_email),
            ("author.date", &fn_author_date),
            ("committer.name", &fn_committer_name),
            ("committer.email", &fn_committer_email),
            ("committer.date", &fn_committer_date),
            ("exactly", &fn_exactly),
            ("current", &fn_current),
            ("merges", &fn_merges),
            ("tests.passed", &fn_tests_passed),
            ("tests.failed", &fn_tests_failed),
            ("tests.fixable", &fn_tests_fixable),
        ];
        functions.iter().cloned().collect()
    };
}

#[instrument]
fn fn_all(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    eval0(ctx, name, args)?;
    let visible_heads = ctx
        .dag
        .query_visible_heads()
        .map_err(EvalError::OtherError)?;
    let visible_commits = ctx.dag.query_ancestors(visible_heads.clone())?;
    let visible_commits = ctx
        .dag
        .filter_visible_commits(visible_commits)
        .map_err(EvalError::OtherError)?;
    Ok(visible_commits)
}

#[instrument]
fn fn_none(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    eval0(ctx, name, args)?;
    Ok(CommitSet::empty())
}

#[instrument]
fn fn_union(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, rhs) = eval2(ctx, name, args)?;
    Ok(lhs.union(&rhs))
}

#[instrument]
fn fn_intersection(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, rhs) = eval2(ctx, name, args)?;
    Ok(lhs.intersection(&rhs))
}

#[instrument]
fn fn_difference(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, rhs) = eval2(ctx, name, args)?;
    Ok(lhs.difference(&rhs))
}

#[instrument]
fn fn_only(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, rhs) = eval2(ctx, name, args)?;
    Ok(ctx.dag.query_only(lhs, rhs)?)
}

#[instrument]
fn fn_range(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, rhs) = eval2(ctx, name, args)?;
    Ok(ctx.dag.query_range(lhs, rhs)?)
}

#[instrument]
fn fn_not(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    let visible_heads = ctx
        .dag
        .query_visible_heads()
        .map_err(EvalError::OtherError)?;
    let visible_commits = ctx.dag.query_ancestors(visible_heads.clone())?;
    Ok(visible_commits.difference(&expr))
}

#[instrument]
fn fn_ancestors(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query_ancestors(expr)?)
}

#[instrument]
fn fn_descendants(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query_descendants(expr)?)
}

#[instrument]
fn fn_parents(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query_parents(expr)?)
}

#[instrument]
fn fn_children(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query_children(expr)?)
}

#[instrument]
fn fn_siblings(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    let parents = ctx.dag.query_parents(expr.clone())?;
    let children = ctx.dag.query_children(parents)?;
    let siblings = children.difference(&expr);
    Ok(siblings)
}

#[instrument]
fn fn_roots(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query_roots(expr)?)
}

#[instrument]
fn fn_heads(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query_heads(expr)?)
}

#[instrument]
fn fn_branches(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = match eval0_or_1_pattern(ctx, name, args)? {
        Some(pattern) => pattern,
        None => return Ok(ctx.dag.branch_commits.clone()),
    };

    let branch_oid_to_names = ctx
        .repo
        .get_references_snapshot()
        .wrap_err("Could not get references snapshot for repo")
        .map_err(EvalError::OtherError)?
        .branch_oid_to_names;

    let branch_commits = make_pattern_matcher_for_set(
        ctx,
        name,
        args,
        Box::new(move |_repo, commit| {
            let branches_at_commit = match branch_oid_to_names.get(&commit.get_oid()) {
                Some(branches) => branches,
                None => return Ok(false),
            };

            let result = branches_at_commit
                .iter()
                .filter_map(
                    |branch_name| match CategorizedReferenceName::new(branch_name) {
                        name @ CategorizedReferenceName::LocalBranch { .. } => {
                            Some(name.render_suffix())
                        }
                        // we only care about local branches
                        CategorizedReferenceName::RemoteBranch { .. }
                        | CategorizedReferenceName::OtherRef { .. } => None,
                    },
                )
                .any(|branch_name| pattern.matches_text(branch_name.as_str()));

            Ok(result)
        }),
        Some(ctx.dag.branch_commits.clone()),
    )?;

    Ok(branch_commits)
}

#[instrument]
fn fn_parents_nth(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, n) = eval_number_rhs(ctx, name, args)?;
    let commit_oids = ctx
        .dag
        .commit_set_to_vec(&lhs)
        .map_err(EvalError::OtherError)?;
    let mut result = Vec::new();
    for commit_oid in commit_oids {
        if let Some(n) = n.checked_sub(1) {
            let parents = ctx.dag.query_parent_names(commit_oid)?;
            if let Some(parent) = parents.get(n) {
                result.push(Ok(parent.clone()))
            }
        }
    }
    Ok(CommitSet::from_iter(result.into_iter(), Hints::default()))
}

#[instrument]
fn fn_nthancestor(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, n) = eval_number_rhs(ctx, name, args)?;
    let commit_oids = ctx
        .dag
        .commit_set_to_vec(&lhs)
        .map_err(EvalError::OtherError)?;
    let n: u64 = u64::try_from(n).unwrap();
    let mut result = Vec::new();
    for commit_oid in commit_oids {
        let ancestor = ctx.dag.query_first_ancestor_nth(commit_oid.into(), n)?;
        if let Some(ancestor) = ancestor {
            result.push(Ok(ancestor.clone()))
        }
    }
    Ok(CommitSet::from_iter(result.into_iter(), Hints::default()))
}

#[instrument]
fn fn_main(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    eval0(ctx, name, args)?;
    Ok(ctx.dag.main_branch_commit.clone())
}

#[instrument]
fn fn_public(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    eval0(ctx, name, args)?;
    let public_commits = ctx
        .dag
        .query_public_commits_slow()
        .map_err(EvalError::OtherError)?;
    Ok(public_commits.clone())
}

#[instrument]
fn fn_draft(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    eval0(ctx, name, args)?;
    let draft_commits = ctx
        .dag
        .query_draft_commits()
        .map_err(EvalError::OtherError)?;
    Ok(draft_commits.clone())
}

#[instrument]
fn fn_stack(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let arg = eval0_or_1(ctx, name, args)?.unwrap_or_else(|| ctx.dag.head_commit.clone());
    ctx.dag
        .query_stack_commits(arg)
        .map_err(EvalError::OtherError)
}

type MatcherFn = dyn Fn(&Repo, &Commit) -> Result<bool, PatternError> + Sync + Send;

/// Make a pattern matcher that operates on all visible commits.
fn make_pattern_matcher(
    ctx: &mut Context,
    name: &str,
    args: &[Expr],
    f: Box<MatcherFn>,
) -> Result<CommitSet, EvalError> {
    make_pattern_matcher_for_set(ctx, name, args, f, None)
}

/// Make a pattern matcher that operates only on the given set of commits.
#[instrument(skip(f))]
fn make_pattern_matcher_for_set(
    ctx: &mut Context,
    name: &str,
    args: &[Expr],
    f: Box<MatcherFn>,
    commits_to_match: Option<CommitSet>,
) -> Result<CommitSet, EvalError> {
    struct Matcher {
        expr: String,
        f: Box<MatcherFn>,
    }

    impl PatternMatcher for Matcher {
        fn get_description(&self) -> &str {
            &self.expr
        }

        fn matches_commit(&self, repo: &Repo, commit: &Commit) -> Result<bool, PatternError> {
            (self.f)(repo, commit)
        }
    }

    let matcher = Matcher {
        expr: Expr::FunctionCall(Cow::Borrowed(name), args.to_vec()).to_string(),
        f,
    };
    let matcher = make_pattern_matcher_set(ctx, ctx.repo, Box::new(matcher), commits_to_match)?;
    Ok(matcher)
}

#[instrument]
fn fn_message(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = eval1_pattern(ctx, name, args)?;
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(move |_repo, commit| {
            let message = commit.get_message_raw();
            let message = match message.to_str() {
                Ok(message) => message,
                Err(err) => {
                    warn!(
                        ?commit,
                        ?message,
                        ?err,
                        "Commit message could not be decoded as UTF-8"
                    );
                    return Ok(false);
                }
            };
            Ok(pattern.matches_text(message))
        }),
    )
}

#[instrument]
fn fn_path_changed(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = eval1_pattern(ctx, name, args)?;
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(move |repo: &Repo, commit: &Commit| {
            let touched_paths = repo
                .get_paths_touched_by_commit(commit)
                .map_err(PatternError::Repo)?;
            let result = touched_paths.into_iter().any(|path| {
                let path = match path.to_str() {
                    Some(path) => path,
                    None => {
                        warn!(
                            ?commit,
                            ?path,
                            "Commit message could not be decoded as UTF-8"
                        );
                        return false;
                    }
                };
                pattern.matches_text(path)
            });
            Ok(result)
        }),
    )
}

#[instrument]
fn fn_author_name(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = eval1_pattern(ctx, name, args)?;
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(
            move |_repo: &Repo, commit: &Commit| match commit.get_author().get_name() {
                Some(name) => Ok(pattern.matches_text(name)),
                None => Ok(false),
            },
        ),
    )
}

#[instrument]
fn fn_author_email(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = eval1_pattern(ctx, name, args)?;
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(
            move |_repo: &Repo, commit: &Commit| match commit.get_author().get_email() {
                Some(name) => Ok(pattern.matches_text(name)),
                None => Ok(false),
            },
        ),
    )
}

#[instrument]
fn fn_author_date(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = eval1_pattern(ctx, name, args)?;
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(move |_repo: &Repo, commit: &Commit| {
            let time = commit.get_author().get_time();
            Ok(pattern.matches_date(&time))
        }),
    )
}

#[instrument]
fn fn_committer_name(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = eval1_pattern(ctx, name, args)?;
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(
            move |_repo: &Repo, commit: &Commit| match commit.get_committer().get_name() {
                Some(name) => Ok(pattern.matches_text(name)),
                None => Ok(false),
            },
        ),
    )
}

#[instrument]
fn fn_committer_email(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = eval1_pattern(ctx, name, args)?;
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(
            move |_repo: &Repo, commit: &Commit| match commit.get_committer().get_email() {
                Some(name) => Ok(pattern.matches_text(name)),
                None => Ok(false),
            },
        ),
    )
}

#[instrument]
fn fn_committer_date(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = eval1_pattern(ctx, name, args)?;
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(move |_repo: &Repo, commit: &Commit| {
            let time = commit.get_committer().get_time();
            Ok(pattern.matches_date(&time))
        }),
    )
}

#[instrument]
fn fn_exactly(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, expected_len) = eval_number_rhs(ctx, name, args)?;
    let actual_len: usize = ctx.dag.set_count(&lhs)?;

    if actual_len == expected_len {
        Ok(lhs)
    } else {
        Err(EvalError::UnexpectedSetLength {
            expr: format!("{}", args[0]),
            expected_len,
            actual_len,
        })
    }
}

#[instrument]
fn fn_current(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let mut dag = ctx
        .dag
        .clear_obsolete_commits(ctx.repo)
        .map_err(EvalError::OtherError)?;
    let mut ctx = Context {
        effects: ctx.effects,
        repo: ctx.repo,
        dag: &mut dag,
    };
    let expr = eval1(&mut ctx, name, args)?;

    let conn = ctx.repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)
        .wrap_err("Connecting to event log")
        .map_err(EvalError::OtherError)?;
    let event_replayer = EventReplayer::from_event_log_db(ctx.effects, ctx.repo, &event_log_db)
        .wrap_err("Retrieving event replayer")
        .map_err(EvalError::OtherError)?;
    let event_cursor = event_replayer.make_default_cursor();

    let commit_oids = ctx
        .dag
        .commit_set_to_vec(&expr)
        .map_err(EvalError::OtherError)?;
    let mut result = Vec::new();
    for commit_oid in commit_oids {
        match find_rewrite_target(&event_replayer, event_cursor, commit_oid) {
            Some(new_commit_oid) => match new_commit_oid {
                MaybeZeroOid::NonZero(new_commit_oid) => {
                    // commit rewritten as new_commit_oid
                    result.push(new_commit_oid);
                }
                MaybeZeroOid::Zero => {
                    // commit deleted, skip
                }
            },
            // commit not rewritten
            None => result.push(commit_oid),
        }
    }
    Ok(result.into_iter().collect::<CommitSet>())
}

#[instrument]
fn fn_merges(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    eval0(ctx, name, args)?;
    // Use a "pattern matcher" that – instead of testing for a pattern –
    // examines the parent count of each commit to find merges.
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(move |_repo, commit| Ok(commit.get_parent_count() > 1)),
    )
}

fn read_all_test_results(repo: &Repo, commit: &Commit) -> Option<Vec<SerializedTestResult>> {
    let commit_test_dir = get_test_tree_dir(repo, commit).ok()?;
    let mut all_results = Vec::new();
    for dir in std::fs::read_dir(commit_test_dir).ok()? {
        let dir = dir.ok()?;
        if dir.file_type().ok()?.is_dir() {
            let result_path = dir.path().join("result");
            let result_contents = std::fs::read_to_string(result_path).ok()?;
            let result: SerializedTestResult = serde_json::from_str(&result_contents).ok()?;
            all_results.push(result);
        }
    }
    Some(all_results)
}

fn read_latest_test_command(repo: &Repo) -> Option<String> {
    let latest_command_path = get_latest_test_command_path(repo).ok()?;
    let latest_command = std::fs::read_to_string(latest_command_path).ok()?;
    Some(latest_command)
}

fn eval_test_command_pattern(
    ctx: &mut Context,
    name: &str,
    args: &[Expr],
) -> Result<Pattern, EvalError> {
    match eval0_or_1_pattern(ctx, name, args)? {
        Some(pattern) => Ok(pattern),
        None => {
            let latest_test_command =
                read_latest_test_command(ctx.repo).ok_or(EvalError::NoLatestTestCommand)?;
            Ok(Pattern::Exact(latest_test_command))
        }
    }
}

#[instrument]
fn fn_tests_passed(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = eval_test_command_pattern(ctx, name, args)?;
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(move |repo: &Repo, commit: &Commit| {
            let result = read_all_test_results(repo, commit)
                .unwrap_or_default()
                .into_iter()
                .any(|test_result| {
                    let SerializedTestResult {
                        command,
                        exit_code,
                        head_commit_oid: _,
                        snapshot_tree_oid: _,
                        interactive: _,
                    } = test_result;
                    exit_code == TEST_SUCCESS_EXIT_CODE
                        && pattern.matches_text(&command.to_string())
                });
            Ok(result)
        }),
    )
}

#[instrument]
fn fn_tests_failed(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = eval_test_command_pattern(ctx, name, args)?;
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(move |repo: &Repo, commit: &Commit| {
            let result = read_all_test_results(repo, commit)
                .unwrap_or_default()
                .into_iter()
                .any(|test_result| {
                    let SerializedTestResult {
                        command,
                        exit_code,
                        head_commit_oid: _,
                        snapshot_tree_oid: _,
                        interactive: _,
                    } = test_result;
                    exit_code != TEST_SUCCESS_EXIT_CODE
                        && exit_code != TEST_INDETERMINATE_EXIT_CODE
                        && exit_code != TEST_ABORT_EXIT_CODE
                        && pattern.matches_text(&command.to_string())
                });
            Ok(result)
        }),
    )
}

#[instrument]
fn fn_tests_fixable(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = eval_test_command_pattern(ctx, name, args)?;
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(move |repo: &Repo, commit: &Commit| {
            let result = read_all_test_results(repo, commit)
                .unwrap_or_default()
                .into_iter()
                .any(|test_result| {
                    let SerializedTestResult {
                        command,
                        exit_code,
                        head_commit_oid: _,
                        snapshot_tree_oid,
                        interactive: _,
                    } = test_result;
                    exit_code == TEST_SUCCESS_EXIT_CODE
                        && pattern.matches_text(&command.to_string())
                        && match (snapshot_tree_oid, commit.get_tree_oid()) {
                            (
                                Some(SerializedNonZeroOid(snapshot_tree_oid)),
                                MaybeZeroOid::NonZero(original_tree_oid),
                            ) => snapshot_tree_oid != original_tree_oid,
                            (None, _) | (_, MaybeZeroOid::Zero) => false,
                        }
                });
            Ok(result)
        }),
    )
}
