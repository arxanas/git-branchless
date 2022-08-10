use eden_dag::DagAlgorithm;
use lib::core::dag::CommitSet;
use lib::git::{Commit, Repo};
use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::TryFrom;
use tracing::warn;

use eyre::Context as EyreContext;
use lazy_static::lazy_static;

use crate::revset::pattern::{PatternError, PatternMatcher};

use super::eval::{
    eval0, eval0_or_1, eval1, eval1_pattern, eval2, eval_number_rhs, Context, EvalError, EvalResult,
};
use super::pattern::make_pattern_matcher_set;
use super::Expr;

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
            ("descendants", &fn_descendants),
            ("parents", &fn_parents),
            ("children", &fn_children),
            ("roots", &fn_roots),
            ("heads", &fn_heads),
            ("branches", &fn_branches),
            ("nthparent", &fn_nthparent),
            ("nthancestor", &fn_nthancestor),
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
        ];
        functions.iter().cloned().collect()
    };
}

fn fn_all(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    eval0(ctx, name, args)?;
    Ok(ctx.query_active_commits()?.clone())
}

fn fn_none(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    eval0(ctx, name, args)?;
    Ok(CommitSet::empty())
}

fn fn_union(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, rhs) = eval2(ctx, name, args)?;
    Ok(lhs.union(&rhs))
}

fn fn_intersection(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, rhs) = eval2(ctx, name, args)?;
    Ok(lhs.intersection(&rhs))
}

fn fn_difference(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, rhs) = eval2(ctx, name, args)?;
    Ok(lhs.difference(&rhs))
}

fn fn_only(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, rhs) = eval2(ctx, name, args)?;
    Ok(ctx.dag.query().only(lhs, rhs)?)
}

fn fn_range(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let (lhs, rhs) = eval2(ctx, name, args)?;
    Ok(ctx.dag.query().range(lhs, rhs)?)
}

fn fn_not(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    let active_commits = ctx.query_active_commits()?;
    Ok(active_commits.difference(&expr))
}

fn fn_ancestors(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query().ancestors(expr)?)
}

fn fn_descendants(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query().descendants(expr)?)
}

fn fn_parents(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query().parents(expr)?)
}

fn fn_children(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query().children(expr)?)
}

fn fn_roots(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query().roots(expr)?)
}

fn fn_heads(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let expr = eval1(ctx, name, args)?;
    Ok(ctx.dag.query().heads(expr)?)
}

fn fn_branches(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    eval0(ctx, name, args)?;
    Ok(ctx.dag.branch_commits.clone())
}

fn fn_nthparent(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
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

fn fn_draft(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    eval0(ctx, name, args)?;
    let draft_commits = ctx.query_draft_commits()?;
    Ok(draft_commits.clone())
}

fn fn_stack(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let arg = eval0_or_1(ctx, name, args)?.unwrap_or_else(|| ctx.dag.head_commit.clone());
    let draft_commits = ctx.query_draft_commits()?;
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

fn fn_message(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let pattern = eval1_pattern(ctx, name, args)?;
    make_pattern_matcher(
        ctx,
        name,
        args,
        Box::new(move |_repo, commit| {
            let message = commit.get_message_raw().map_err(PatternError::Repo)?;
            let message = match message.to_str() {
                Some(message) => message,
                None => {
                    warn!(
                        ?commit,
                        ?message,
                        "Commit message could not be decoded as UTF-8"
                    );
                    return Ok(false);
                }
            };
            Ok(pattern.matches_text(message))
        }),
    )
}

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
