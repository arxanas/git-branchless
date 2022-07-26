use std::collections::HashMap;
use std::sync::Arc;

use eden_dag::DagAlgorithm;
use itertools::Itertools;
use lazy_static::lazy_static;
use lib::core::effects::{Effects, OperationType};
use once_cell::unsync::OnceCell;
use thiserror::Error;

use lib::core::dag::{CommitSet, Dag};
use lib::git::{Repo, ResolvedReferenceInfo};
use tracing::instrument;

use super::Expr;

#[derive(Debug)]
struct Context<'a> {
    effects: &'a Effects,
    repo: &'a Repo,
    dag: &'a mut Dag,
    public_commits: OnceCell<CommitSet>,
    active_heads: OnceCell<CommitSet>,
    active_commits: OnceCell<CommitSet>,
    draft_commits: OnceCell<CommitSet>,
}

impl Context<'_> {
    #[instrument]
    pub fn query_public_commits(&self) -> Result<&CommitSet, EvalError> {
        self.public_commits.get_or_try_init(|| {
            let public_commits = self
                .dag
                .query_public_commits()
                .map_err(EvalError::OtherError)?;
            Ok(public_commits)
        })
    }

    #[instrument]
    pub fn query_active_heads(&self) -> Result<&CommitSet, EvalError> {
        self.active_heads.get_or_try_init(|| {
            let public_commits = self.query_public_commits()?;
            let active_heads = self
                .dag
                .query_active_heads(
                    public_commits,
                    &self
                        .dag
                        .observed_commits
                        .difference(&self.dag.obsolete_commits),
                )
                .map_err(EvalError::OtherError)?;
            Ok(active_heads)
        })
    }

    #[instrument]
    pub fn query_active_commits(&self) -> Result<&CommitSet, EvalError> {
        self.active_commits.get_or_try_init(|| {
            let active_heads = self.query_active_heads()?;
            let active_commits = self.dag.query().ancestors(active_heads.clone())?;
            Ok(active_commits)
        })
    }

    #[instrument]
    pub fn query_draft_commits(&self) -> Result<&CommitSet, EvalError> {
        self.draft_commits.get_or_try_init(|| {
            let public_commits = self.query_public_commits()?;
            let active_heads = self.query_active_heads()?;
            Ok(self
                .dag
                .query()
                .only(active_heads.clone(), public_commits.clone())?)
        })
    }
}

#[derive(Debug, Error)]
pub enum EvalError {
    #[error("no commit, branch, or reference with the name '{name}' could be found")]
    UnboundName { name: String },

    #[error(
        "no function with the name '{name}' could be found; these functions are available: {}",
        available_names.join(", "),
    )]
    UnboundFunction {
        name: String,
        available_names: Vec<&'static str>,
    },

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

    #[error(transparent)]
    OtherError(eyre::Error),
}

pub type EvalResult = Result<CommitSet, EvalError>;

/// Evaluate the provided revset expression.
#[instrument]
pub fn eval(effects: &Effects, repo: &Repo, dag: &mut Dag, expr: &Expr) -> EvalResult {
    let (effects, _progress) =
        effects.start_operation(OperationType::EvaluateRevset(Arc::new(expr.to_string())));

    let mut ctx = Context {
        effects: &effects,
        repo,
        dag,
        public_commits: Default::default(),
        active_heads: Default::default(),
        active_commits: Default::default(),
        draft_commits: Default::default(),
    };
    let commits = eval_inner(&mut ctx, expr)?;
    Ok(commits)
}

#[instrument]
fn eval_inner(ctx: &mut Context, expr: &Expr) -> EvalResult {
    match expr {
        Expr::Name(name) => eval_name(ctx, name),
        Expr::Fn(name, args) => eval_fn(ctx, name, args),
    }
}

fn eval_name(ctx: &mut Context, name: &str) -> EvalResult {
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
    let commit_set = match commit {
        Ok(Some(commit)) => {
            let commit_set: CommitSet = commit.get_oid().into();
            commit_set
        }
        Ok(None) | Err(_) => {
            return Err(EvalError::UnboundName {
                name: name.to_owned(),
            })
        }
    };

    ctx.dag
        .sync_from_oids(
            ctx.effects,
            ctx.repo,
            CommitSet::empty(),
            commit_set.clone(),
        )
        .map_err(EvalError::OtherError)?;
    Ok(commit_set)
}

fn eval_fn(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
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
    fn fn_negate(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
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
    fn fn_draft(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
        eval0(ctx, name, args)?;
        let draft_commits = ctx.query_draft_commits()?;
        Ok(draft_commits.clone())
    }
    fn fn_stack(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
        eval0(ctx, name, args)?;
        let draft_commits = ctx.query_draft_commits()?;
        let stack_roots = ctx.dag.query().roots(draft_commits.clone())?;
        let stack_ancestors = ctx
            .dag
            .query()
            .range(stack_roots, ctx.dag.head_commit.clone())?;
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

    type FnType = &'static (dyn Fn(&mut Context, &str, &[Expr]) -> EvalResult + Sync);
    lazy_static! {
        static ref FUNCTIONS: HashMap<&'static str, FnType> = {
            let functions: &[(&'static str, FnType)] = &[
                ("union", &fn_union),
                ("intersection", &fn_intersection),
                ("difference", &fn_difference),
                ("only", &fn_only),
                ("range", &fn_range),
                ("negate", &fn_negate),
                ("ancestors", &fn_ancestors),
                ("descendants", &fn_descendants),
                ("parents", &fn_parents),
                ("children", &fn_children),
                ("roots", &fn_roots),
                ("heads", &fn_heads),
                ("branches", &fn_branches),
                ("draft", &fn_draft),
                ("stack", &fn_stack),
            ];
            functions.iter().cloned().collect()
        };
    }

    let function = FUNCTIONS
        .get(name)
        .ok_or_else(|| EvalError::UnboundFunction {
            name: name.to_owned(),
            available_names: FUNCTIONS.keys().sorted().copied().collect(),
        })?;

    function(ctx, name, args)
}

#[instrument]
fn eval0(ctx: &mut Context, function_name: &str, args: &[Expr]) -> Result<(), EvalError> {
    match args {
        [] => Ok(()),

        args => Err(EvalError::ArityMismatch {
            function_name: function_name.to_string(),
            expected_arity: 0,
            actual_arity: args.len(),
        }),
    }
}

#[instrument]
fn eval1(ctx: &mut Context, function_name: &str, args: &[Expr]) -> EvalResult {
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

    use lib::core::dag::commit_set_to_vec_unsorted;
    use lib::core::effects::Effects;
    use lib::core::eventlog::{EventLogDb, EventReplayer};
    use lib::core::formatting::Glyphs;
    use lib::core::repo_ext::RepoExt;
    use lib::git::Commit;
    use lib::testing::make_git;

    use super::*;
    use crate::revset::Expr;

    fn eval_and_sort<'a>(
        effects: &Effects,
        repo: &'a Repo,
        dag: &mut Dag,
        expr: &Expr,
    ) -> eyre::Result<Vec<Commit<'a>>> {
        let result = eval(effects, repo, dag, expr)?;
        let mut commits: Vec<Commit> = commit_set_to_vec_unsorted(&result)?
            .into_iter()
            .map(|oid| repo.find_commit_or_fail(oid))
            .try_collect()?;
        commits.sort_by_key(|commit| (commit.get_message_pretty().unwrap(), commit.get_time()));
        Ok(commits)
    }

    #[test]
    fn test_eval() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        let test1_oid = git.commit_file("test1", 1)?;
        git.detach_head()?;
        let test2_oid = git.commit_file("test2", 2)?;
        let _test3_oid = git.commit_file("test3", 3)?;

        git.run(&["checkout", "master"])?;
        git.commit_file("test4", 4)?;
        git.detach_head()?;
        git.commit_file("test5", 5)?;
        git.commit_file("test6", 6)?;
        git.run(&["checkout", "HEAD~"])?;
        git.commit_file("test7", 7)?;

        let effects = Effects::new_suppress_for_test(Glyphs::text());
        let repo = git.get_repo()?;
        let conn = repo.get_db_conn()?;
        let event_log_db = EventLogDb::new(&conn)?;
        let event_replayer = EventReplayer::from_event_log_db(&effects, &repo, &event_log_db)?;
        let event_cursor = event_replayer.make_default_cursor();
        let references_snapshot = repo.get_references_snapshot()?;
        let mut dag = Dag::open_and_sync(
            &effects,
            &repo,
            &event_replayer,
            event_cursor,
            &references_snapshot,
        )?;

        {
            let expr = Expr::Fn(
                Cow::Borrowed("union"),
                vec![
                    Expr::Name(test1_oid.to_string()),
                    Expr::Name(test2_oid.to_string()),
                ],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [
                    Commit {
                        inner: Commit {
                            id: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                            summary: "create test1.txt",
                        },
                    },
                    Commit {
                        inner: Commit {
                            id: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                            summary: "create test2.txt",
                        },
                    },
                ],
            )
            "###);
        }

        {
            let expr = Expr::Fn(Cow::Borrowed("stack"), vec![]);
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [
                    Commit {
                        inner: Commit {
                            id: 848121cb21bf9af8b064c91bc8930bd16d624a22,
                            summary: "create test5.txt",
                        },
                    },
                    Commit {
                        inner: Commit {
                            id: f0abf649939928fe5475179fd84e738d3d3725dc,
                            summary: "create test6.txt",
                        },
                    },
                    Commit {
                        inner: Commit {
                            id: ba07500a4adc661dc06a748d200ef92120e1b355,
                            summary: "create test7.txt",
                        },
                    },
                ],
            )
            "###);
        }

        {
            let expr = Expr::Fn(Cow::Borrowed("draft"), vec![]);
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [
                    Commit {
                        inner: Commit {
                            id: 96d1c37a3d4363611c49f7e52186e189a04c531f,
                            summary: "create test2.txt",
                        },
                    },
                    Commit {
                        inner: Commit {
                            id: 70deb1e28791d8e7dd5a1f0c871a51b91282562f,
                            summary: "create test3.txt",
                        },
                    },
                    Commit {
                        inner: Commit {
                            id: 848121cb21bf9af8b064c91bc8930bd16d624a22,
                            summary: "create test5.txt",
                        },
                    },
                    Commit {
                        inner: Commit {
                            id: f0abf649939928fe5475179fd84e738d3d3725dc,
                            summary: "create test6.txt",
                        },
                    },
                    Commit {
                        inner: Commit {
                            id: ba07500a4adc661dc06a748d200ef92120e1b355,
                            summary: "create test7.txt",
                        },
                    },
                ],
            )
            "###);
        }

        {
            let expr = Expr::Fn(
                Cow::Borrowed("negate"),
                vec![Expr::Fn(Cow::Borrowed("draft"), vec![])],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [
                    Commit {
                        inner: Commit {
                            id: f777ecc9b0db5ed372b2615695191a8a17f79f24,
                            summary: "create initial.txt",
                        },
                    },
                    Commit {
                        inner: Commit {
                            id: 62fc20d2a290daea0d52bdc2ed2ad4be6491010e,
                            summary: "create test1.txt",
                        },
                    },
                    Commit {
                        inner: Commit {
                            id: bf0d52a607f693201512a43b6b5a70b2a275e0ad,
                            summary: "create test4.txt",
                        },
                    },
                ],
            )
            "###);
        }

        Ok(())
    }
}
