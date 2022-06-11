use std::borrow::Borrow;

use eden_dag::DagAlgorithm;
use thiserror::Error;

use lib::core::dag::{CommitSet, Dag};
use lib::git::{Repo, ResolvedReferenceInfo};
use tracing::instrument;

use super::Expr;

#[derive(Debug)]
struct Context<'a> {
    repo: &'a Repo,
    dag: &'a Dag,
}

#[derive(Debug, Error)]
pub enum EvalError {
    #[error("name is not defined: '{name}'")]
    UnboundName { name: String },

    #[error("invalid number of arguments to {function_name}: expected {expected_arity} but got {actual_arity}")]
    ArityMismatch {
        function_name: String,
        expected_arity: usize,
        actual_arity: usize,
    },

    #[error("query error: {from}")]
    DagError {
        #[from]
        from: eden_dag::Error,
    },
}

pub type EvalResult = Result<CommitSet, EvalError>;

/// Evaluate the provided revset expression.
#[instrument]
pub fn eval(repo: &Repo, dag: &Dag, expr: &Expr) -> EvalResult {
    let mut ctx = Context { repo, dag };
    eval_inner(&mut ctx, expr)
}

#[instrument]
fn eval_inner(ctx: &mut Context, expr: &Expr) -> EvalResult {
    match expr {
        Expr::Name(name) => eval_name(ctx, name),

        Expr::Fn(name, args) => match name.borrow() {
            "union" => {
                let (lhs, rhs) = eval2(ctx, name, args)?;
                Ok(lhs.union(&rhs))
            }

            "intersection" => {
                let (lhs, rhs) = eval2(ctx, name, args)?;
                Ok(lhs.intersection(&rhs))
            }

            "difference" => {
                let (lhs, rhs) = eval2(ctx, name, args)?;
                Ok(lhs.difference(&rhs))
            }

            "only" => {
                let (lhs, rhs) = eval2(ctx, name, args)?;
                Ok(ctx.dag.query().only(lhs, rhs)?)
            }

            "range" => {
                let (lhs, rhs) = eval2(ctx, name, args)?;
                Ok(ctx.dag.query().range(lhs, rhs)?)
            }

            "ancestors" => {
                let expr = eval1(ctx, name, args)?;
                Ok(ctx.dag.query().ancestors(expr)?)
            }

            "descendants" => {
                let expr = eval1(ctx, name, args)?;
                Ok(ctx.dag.query().descendants(expr)?)
            }

            "parents" => {
                let expr = eval1(ctx, name, args)?;
                Ok(ctx.dag.query().parents(expr)?)
            }

            name => Err(EvalError::UnboundName {
                name: name.to_owned(),
            }),
        },
    }
}

fn eval_name(ctx: &mut Context, name: &str) -> Result<CommitSet, EvalError> {
    if name == "." || name == "@" {
        let head_info = ctx.repo.get_head_info();
        if let Ok(ResolvedReferenceInfo {
            oid: Some(oid),
            reference_name: _,
        }) = head_info
        {
            return Ok(oid.into());
        }
    }

    let commit = ctx.repo.revparse_single_commit(name);
    match commit {
        Ok(Some(commit)) => {
            let commit_set: CommitSet = commit.get_oid().into();
            Ok(commit_set)
        }
        Ok(None) | Err(_) => Err(EvalError::UnboundName {
            name: name.to_owned(),
        }),
    }
}

#[instrument]
fn eval1(ctx: &mut Context, function_name: &str, args: &[Expr]) -> Result<CommitSet, EvalError> {
    match args {
        [expr] => {
            let lhs = eval_inner(ctx, expr)?;
            Ok(lhs)
        }

        args => Err(EvalError::ArityMismatch {
            function_name: function_name.to_string(),
            expected_arity: 1,
            actual_arity: args.len(),
        }),
    }
}

#[instrument]
fn eval2(
    ctx: &mut Context,
    function_name: &str,
    args: &[Expr],
) -> Result<(CommitSet, CommitSet), EvalError> {
    match args {
        [lhs, rhs] => {
            let lhs = eval_inner(ctx, lhs)?;
            let rhs = eval_inner(ctx, rhs)?;
            Ok((lhs, rhs))
        }

        args => Err(EvalError::ArityMismatch {
            function_name: function_name.to_string(),
            expected_arity: 2,
            actual_arity: args.len(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use lib::core::effects::Effects;
    use lib::core::eventlog::{EventLogDb, EventReplayer};
    use lib::core::formatting::Glyphs;
    use lib::core::repo_ext::RepoExt;
    use lib::testing::make_git;

    use super::*;
    use crate::revset::Expr;

    #[test]
    fn test_eval() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        let test1_oid = git.commit_file("test1", 1)?;
        let test2_oid = git.commit_file("test2", 2)?;
        let _test3_oid = git.commit_file("test3", 3)?;

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

        let expr = Expr::Fn(
            Cow::Borrowed("union"),
            vec![
                Expr::Name(test1_oid.to_string()),
                Expr::Name(test2_oid.to_string()),
            ],
        );
        insta::assert_debug_snapshot!(eval(&repo, &dag, &expr), @r###"
        Ok(
            <or
              <static [
                  62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
              ]>
              <static [
                  96d1c37a3d4363611c49f7e52186e189a04c531f,
              ]>>,
        )
        "###);

        Ok(())
    }
}
