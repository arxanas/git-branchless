use bstr::ByteSlice;
use eden_dag::DagAlgorithm;
use lib::core::dag::CommitSet;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::rewrite::find_rewrite_target;
use lib::git::{Commit, MaybeZeroOid, NonZeroOid, Repo};
use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::TryFrom;
use tracing::{instrument, warn};

use eyre::Context as EyreContext;
use lazy_static::lazy_static;

use crate::eval::{
    eval0, eval0_or_1, eval1, eval1_pattern, eval2, eval_number_rhs, Context, EvalError, EvalResult,
};
use crate::pattern::make_pattern_matcher_set;
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
    let visible_commits = ctx.dag.query().ancestors(visible_heads.clone())?;
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
    Ok(ctx.dag.query().only(lhs, rhs)?)
}

#[instrument]
fn fn_range(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, rhs) = eval2(ctx, name, args)?;
    Ok(ctx.dag.query().range(lhs, rhs)?)
}

#[instrument]
fn fn_not(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    let visible_heads = ctx
        .dag
        .query_visible_heads()
        .map_err(EvalError::OtherError)?;
    let visible_commits = ctx.dag.query().ancestors(visible_heads.clone())?;
    Ok(visible_commits.difference(&expr))
}

#[instrument]
fn fn_ancestors(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query().ancestors(expr)?)
}

#[instrument]
fn fn_descendants(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query().descendants(expr)?)
}

#[instrument]
fn fn_parents(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query().parents(expr)?)
}

#[instrument]
fn fn_children(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query().children(expr)?)
}

#[instrument]
fn fn_siblings(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    let parents = ctx.dag.query().parents(expr.clone())?;
    let children = ctx.dag.query().children(parents)?;
    let siblings = children.difference(&expr);
    Ok(siblings)
}

#[instrument]
fn fn_roots(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query().roots(expr)?)
}

#[instrument]
fn fn_heads(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query().heads(expr)?)
}

#[instrument]
fn fn_branches(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    eval0(ctx, name, args)?;
    Ok(ctx.dag.branch_commits.clone())
}

#[instrument]
fn fn_parents_nth(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, n) = eval_number_rhs(ctx, name, args)?;
    let mut result = Vec::new();
    for vertex in lhs
        .iter()
        .wrap_err("Iterating commit set")
        .map_err(EvalError::OtherError)?
    {
        let vertex = vertex
            .wrap_err("Evaluating vertex")
            .map_err(EvalError::OtherError)?;
        if let Some(n) = n.checked_sub(1) {
            let parents = ctx.dag.query().parent_names(vertex)?;
            if let Some(parent) = parents.get(n) {
                result.push(Ok(parent.clone()))
            }
        }
    }
    Ok(CommitSet::from_iter(result.into_iter()))
}

#[instrument]
fn fn_nthancestor(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, n) = eval_number_rhs(ctx, name, args)?;
    let n: u64 = u64::try_from(n).unwrap();
    let mut result = Vec::new();
    for vertex in lhs
        .iter()
        .wrap_err("Iterating commit set")
        .map_err(EvalError::OtherError)?
    {
        let vertex = vertex
            .wrap_err("Evaluating vertex")
            .map_err(EvalError::OtherError)?;
        let ancestor = ctx.dag.query().first_ancestor_nth(vertex, n);
        if let Ok(ancestor) = ancestor {
            result.push(Ok(ancestor.clone()))
        }
    }
    Ok(CommitSet::from_iter(result.into_iter()))
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
    let draft_commits = ctx
        .dag
        .query_draft_commits()
        .map_err(EvalError::OtherError)?;
    let stack_roots = ctx.dag.query().roots(draft_commits.clone())?;
    let stack_ancestors = ctx.dag.query().range(stack_roots, arg)?;
    let stack = ctx
        .dag
        .query()
        // Note that for a graph like
        //
        // ```
        // O
        // |
        // o A
        // | \
        // |  o B
        // |
        // @ C
        // ```
        // this will return `{A, B, C}`, not just `{A, C}`.
        .range(stack_ancestors, draft_commits.clone())?;
    Ok(stack)
}

type MatcherFn = dyn Fn(&Repo, &Commit) -> Result<bool, PatternError> + Sync + Send;

#[instrument(skip(f))]
fn make_pattern_matcher(
    ctx: &mut Context,
    name: &str,
    args: &[Expr],
    f: Box<MatcherFn>,
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
    let matcher = make_pattern_matcher_set(ctx, ctx.repo, Box::new(matcher))?;
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
            let message = commit.get_message_raw().map_err(PatternError::Repo)?;
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
            let touched_paths = match repo
                .get_paths_touched_by_commit(commit)
                .map_err(PatternError::Repo)?
            {
                Some(touched_paths) => touched_paths,
                None => {
                    // FIXME: it might be more intuitive to check all changed
                    // paths with respect to any parent.
                    return Ok(false);
                }
            };
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
    let actual_len: usize = lhs
        .count()
        .wrap_err("Counting commit set")
        .map_err(EvalError::OtherError)?;

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

    let mut result = Vec::new();
    for vertex in expr
        .iter()
        .wrap_err("Iterating commit set")
        .map_err(EvalError::OtherError)?
    {
        let vertex = vertex
            .wrap_err("Evaluating vertex")
            .map_err(EvalError::OtherError)?;

        let oid = NonZeroOid::try_from(vertex)
            .wrap_err("Converting vertex to oid")
            .map_err(EvalError::OtherError)?;

        match find_rewrite_target(&event_replayer, event_cursor, oid) {
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
            None => result.push(oid),
        }
    }
    Ok(result.into_iter().collect::<CommitSet>())
}
