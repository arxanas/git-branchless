use eden_dag::DagAlgorithm;
use lib::core::dag::CommitSet;
use std::collections::HashMap;
use std::convert::TryFrom;

use eyre::Context as EyreContext;
use lazy_static::lazy_static;

use super::eval::{
    eval0, eval0_or_1, eval1, eval2, eval_number_rhs, Context, EvalError, EvalResult,
};
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
