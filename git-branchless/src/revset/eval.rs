use std::fmt::Display;
use std::num::ParseIntError;
use std::sync::Arc;

use eden_dag::errors::BackendError;
use eden_dag::DagAlgorithm;
use itertools::Itertools;
use lib::core::effects::{Effects, OperationType};
use once_cell::unsync::OnceCell;
use thiserror::Error;

use lib::core::dag::{CommitSet, Dag};
use lib::core::formatting::Pluralize;
use lib::git::{Repo, ResolvedReferenceInfo};
use tracing::instrument;

use super::builtins::FUNCTIONS;
use super::pattern::{Pattern, PatternError};
use super::Expr;

#[derive(Debug)]
pub(super) struct Context<'a> {
    pub effects: &'a Effects,
    pub repo: &'a Repo,
    pub dag: &'a mut Dag,
    pub public_commits: OnceCell<CommitSet>,
    pub active_heads: OnceCell<CommitSet>,
    pub active_commits: OnceCell<CommitSet>,
    pub draft_commits: OnceCell<CommitSet>,
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

    #[error(
        "invalid number of arguments to {function_name}: expected {} but got {actual_arity}",
        expected_arities.iter().map(|arity| arity.to_string()).collect::<Vec<_>>().join("/"),
    )]
    ArityMismatch {
        function_name: String,
        expected_arities: Vec<usize>,
        actual_arity: usize,
    },

    #[error(
        "expected '{expr}' to evaluate to {}, but got {actual_len}",
        Pluralize {
            determiner: None,
            amount: *expected_len,
            unit: ("element", "elements"),
        }
    )]
    UnexpectedSetLength {
        expr: String,
        expected_len: usize,
        actual_len: usize,
    },

    #[error("not an integer: {from}")]
    ParseInt {
        #[from]
        from: ParseIntError,
    },

    #[error("expected an integer, but got a call to function: {function_name}")]
    ExpectedNumberNotFunction { function_name: String },

    #[error("expected a text-matching pattern, but got a call to function: {function_name}")]
    ExpectedPatternNotFunction { function_name: String },

    #[error(transparent)]
    PatternError(#[from] PatternError),

    #[error("query error: {from}")]
    DagError {
        #[from]
        from: eden_dag::Error,
    },

    #[error(transparent)]
    OtherError(eyre::Error),
}

pub(super) fn make_dag_backend_error(error: impl Display) -> eden_dag::Error {
    let error = format!("error: {}", error);
    let error = BackendError::Generic(error);
    eden_dag::Error::Backend(Box::new(error))
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
        Expr::FunctionCall(name, args) => eval_fn(ctx, name, args),
    }
}

pub(super) fn eval_name(ctx: &mut Context, name: &str) -> EvalResult {
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

pub(super) fn eval_fn(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    let function = FUNCTIONS
        .get(name)
        .ok_or_else(|| EvalError::UnboundFunction {
            name: name.to_owned(),
            available_names: FUNCTIONS.keys().sorted().copied().collect(),
        })?;

    function(ctx, name, args)
}

#[instrument]
pub(super) fn eval0(
    ctx: &mut Context,
    function_name: &str,
    args: &[Expr],
) -> Result<(), EvalError> {
    match args {
        [] => Ok(()),

        args => Err(EvalError::ArityMismatch {
            function_name: function_name.to_string(),
            expected_arities: vec![0],
            actual_arity: args.len(),
        }),
    }
}

#[instrument]
pub(super) fn eval0_or_1(
    ctx: &mut Context,
    function_name: &str,
    args: &[Expr],
) -> Result<Option<CommitSet>, EvalError> {
    match args {
        [] => Ok(None),
        [expr] => {
            let arg = eval_inner(ctx, expr)?;
            Ok(Some(arg))
        }
        args => Err(EvalError::ArityMismatch {
            function_name: function_name.to_string(),
            expected_arities: vec![0, 1],
            actual_arity: args.len(),
        }),
    }
}

#[instrument]
pub(super) fn eval1(ctx: &mut Context, function_name: &str, args: &[Expr]) -> EvalResult {
    match args {
        [arg] => {
            let lhs = eval_inner(ctx, arg)?;
            Ok(lhs)
        }

        args => Err(EvalError::ArityMismatch {
            function_name: function_name.to_string(),
            expected_arities: vec![1],
            actual_arity: args.len(),
        }),
    }
}

pub(super) fn eval1_pattern(
    _ctx: &mut Context,
    function_name: &str,
    args: &[Expr],
) -> Result<Pattern, EvalError> {
    match args {
        [Expr::Name(pattern)] => Ok(Pattern::new(pattern)?),

        [Expr::FunctionCall(name, _args)] => Err(EvalError::ExpectedNumberNotFunction {
            function_name: name.clone().into_owned(),
        }),

        args => Err(EvalError::ArityMismatch {
            function_name: function_name.to_string(),
            expected_arities: vec![1],
            actual_arity: args.len(),
        }),
    }
}

#[instrument]
pub(super) fn eval2(
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
            expected_arities: vec![2],
            actual_arity: args.len(),
        }),
    }
}

pub(super) fn eval_number_rhs(
    ctx: &mut Context,
    function_name: &str,
    args: &[Expr],
) -> Result<(CommitSet, usize), EvalError> {
    match args {
        [lhs, Expr::Name(name)] => {
            let lhs = eval_inner(ctx, lhs)?;
            let number: usize = { name.parse()? };
            Ok((lhs, number))
        }

        [_lhs, Expr::FunctionCall(name, _args)] => Err(EvalError::ExpectedNumberNotFunction {
            function_name: name.clone().into_owned(),
        }),

        args => Err(EvalError::ArityMismatch {
            function_name: function_name.to_string(),
            expected_arities: vec![2],
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
    use lib::testing::{make_git, GitRunOptions};

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
        let test7_oid = git.commit_file("test7", 7)?;

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
            let expr = Expr::FunctionCall(Cow::Borrowed("all"), vec![]);
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
                            id: bf0d52a607f693201512a43b6b5a70b2a275e0ad,
                            summary: "create test4.txt",
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
            let expr = Expr::FunctionCall(Cow::Borrowed("none"), vec![]);
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [],
            )
            "###);
        }

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("union"),
                vec![
                    Expr::Name(Cow::Owned(test1_oid.to_string())),
                    Expr::Name(Cow::Owned(test2_oid.to_string())),
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
            let expr = Expr::FunctionCall(Cow::Borrowed("stack"), vec![]);
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
            let expr = Expr::FunctionCall(
                Cow::Borrowed("stack"),
                vec![Expr::Name(Cow::Owned(test2_oid.to_string()))],
            );
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
                ],
            )
            "###);
        }

        {
            let expr = Expr::FunctionCall(Cow::Borrowed("draft"), vec![]);
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
            let expr = Expr::FunctionCall(
                Cow::Borrowed("not"),
                vec![Expr::FunctionCall(Cow::Borrowed("draft"), vec![])],
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

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("parents.nth"),
                vec![
                    Expr::Name(Cow::Owned(test7_oid.to_string())),
                    Expr::Name(Cow::Borrowed("1")),
                ],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [
                    Commit {
                        inner: Commit {
                            id: 848121cb21bf9af8b064c91bc8930bd16d624a22,
                            summary: "create test5.txt",
                        },
                    },
                ],
            )
            "###);
        }

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("ancestors.nth"),
                vec![
                    Expr::Name(Cow::Owned(test7_oid.to_string())),
                    Expr::Name(Cow::Borrowed("2")),
                ],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [
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

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("message"),
                vec![Expr::Name(Cow::Borrowed("test4"))],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [
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

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("paths.changed"),
                vec![Expr::Name(Cow::Borrowed("glob:test[1-3].txt"))],
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
                    Commit {
                        inner: Commit {
                            id: 70deb1e28791d8e7dd5a1f0c871a51b91282562f,
                            summary: "create test3.txt",
                        },
                    },
                ],
            )
            "###);
        }

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("exactly"),
                vec![
                    Expr::FunctionCall(Cow::Borrowed("stack"), vec![]),
                    Expr::Name(Cow::Borrowed("3")),
                ],
            );
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
            let expr = Expr::FunctionCall(
                Cow::Borrowed("exactly"),
                vec![
                    Expr::FunctionCall(Cow::Borrowed("stack"), vec![]),
                    Expr::Name(Cow::Borrowed("2")),
                ],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Err(
                UnexpectedSetLength {
                    expr: "stack()",
                    expected_len: 2,
                    actual_len: 3,
                },
            )
            "###);
        }

        Ok(())
    }

    #[test]
    fn test_eval_author_committer() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        git.write_file("test1", "test\n")?;
        git.run(&["add", "test1.txt"])?;
        git.run_with_options(
            &["commit", "-m", "test1"],
            &GitRunOptions {
                env: {
                    [
                        ("GIT_AUTHOR_NAME", "Foo"),
                        ("GIT_AUTHOR_EMAIL", "foo@example.com"),
                        ("GIT_COMMITTER_NAME", "Bar"),
                        ("GIT_COMMITTER_EMAIL", "bar@example.com"),
                    ]
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
                },
                ..Default::default()
            },
        )?;

        git.write_file("test2", "test\n")?;
        git.run(&["add", "test2.txt"])?;
        git.run_with_options(
            &["commit", "-m", "test2"],
            &GitRunOptions {
                env: {
                    [
                        ("GIT_AUTHOR_NAME", "Bar"),
                        ("GIT_AUTHOR_EMAIL", "bar@example.com"),
                        ("GIT_COMMITTER_NAME", "Foo"),
                        ("GIT_COMMITTER_EMAIL", "foo@example.com"),
                    ]
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
                },
                ..Default::default()
            },
        )?;

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
            let expr = Expr::FunctionCall(
                Cow::Borrowed("author.name"),
                vec![Expr::Name(Cow::Borrowed("Foo"))],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [
                    Commit {
                        inner: Commit {
                            id: 9ee1994c0737c221efc07acd8d73590d336ee46d,
                            summary: "test1",
                        },
                    },
                ],
            )
            "###);
        }

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("author.email"),
                vec![Expr::Name(Cow::Borrowed("foo"))],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [
                    Commit {
                        inner: Commit {
                            id: 9ee1994c0737c221efc07acd8d73590d336ee46d,
                            summary: "test1",
                        },
                    },
                ],
            )
            "###);
        }

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("author.date"),
                vec![Expr::Name(Cow::Borrowed("before:today"))],
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
                            id: 9ee1994c0737c221efc07acd8d73590d336ee46d,
                            summary: "test1",
                        },
                    },
                    Commit {
                        inner: Commit {
                            id: 05ff2fc6b3e7917ac6800b18077c211e173e8fb4,
                            summary: "test2",
                        },
                    },
                ],
            )
            "###);
        }

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("author.date"),
                vec![Expr::Name(Cow::Borrowed("after:yesterday"))],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [],
            )
            "###);
        }

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("committer.name"),
                vec![Expr::Name(Cow::Borrowed("Foo"))],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [
                    Commit {
                        inner: Commit {
                            id: 05ff2fc6b3e7917ac6800b18077c211e173e8fb4,
                            summary: "test2",
                        },
                    },
                ],
            )
            "###);
        }

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("committer.email"),
                vec![Expr::Name(Cow::Borrowed("foo"))],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [
                    Commit {
                        inner: Commit {
                            id: 05ff2fc6b3e7917ac6800b18077c211e173e8fb4,
                            summary: "test2",
                        },
                    },
                ],
            )
            "###);
        }

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("committer.date"),
                vec![Expr::Name(Cow::Borrowed("before:today"))],
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
                            id: 9ee1994c0737c221efc07acd8d73590d336ee46d,
                            summary: "test1",
                        },
                    },
                    Commit {
                        inner: Commit {
                            id: 05ff2fc6b3e7917ac6800b18077c211e173e8fb4,
                            summary: "test2",
                        },
                    },
                ],
            )
            "###);
        }

        {
            let expr = Expr::FunctionCall(
                Cow::Borrowed("committer.date"),
                vec![Expr::Name(Cow::Borrowed("after:yesterday"))],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [],
            )
            "###);
        }

        Ok(())
    }
}
