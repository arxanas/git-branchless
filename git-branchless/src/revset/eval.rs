use std::collections::HashMap;
use std::fmt::Display;
use std::num::ParseIntError;
use std::sync::Arc;

use eden_dag::errors::BackendError;
use itertools::Itertools;
use lib::core::effects::{Effects, OperationType};
use thiserror::Error;

use lib::core::dag::{CommitSet, Dag};
use lib::core::formatting::Pluralize;
use lib::git::{ConfigRead, Repo, RepoError, ResolvedReferenceInfo};
use tracing::instrument;

use super::builtins::FUNCTIONS;
use super::parser::{parse, ParseError};
use super::pattern::{Pattern, PatternError};
use super::Expr;

#[derive(Debug)]
pub(super) struct Context<'a> {
    pub effects: &'a Effects,
    pub repo: &'a Repo,
    pub dag: &'a mut Dag,
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

    #[error("failed to parse alias expression '{alias}'\n{source}")]
    ParseAlias { alias: String, source: ParseError },

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

    #[error(transparent)]
    RepoError(#[from] RepoError),

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
    };
    let commits = eval_inner(&mut ctx, expr)?;
    Ok(commits)
}

#[instrument]
fn eval_inner(ctx: &mut Context, expr: &Expr) -> EvalResult {
    match expr {
        Expr::Name(name) => eval_name(ctx, name),
        Expr::FunctionCall(name, args) => {
            let result = eval_fn(ctx, name, args)?;
            let result = ctx
                .dag
                .filter_visible_commits(result)
                .map_err(EvalError::OtherError)?;
            Ok(result)
        }
    }
}

#[instrument]
pub(super) fn eval_name(ctx: &mut Context, name: &str) -> EvalResult {
    if name == "." || name == "@" {
        let head_info = ctx.repo.get_head_info()?;
        return match head_info {
            ResolvedReferenceInfo {
                oid: Some(oid),
                reference_name: _,
            } => Ok(oid.into()),
            ResolvedReferenceInfo {
                oid: None,
                reference_name: _,
            } => Ok(CommitSet::empty()),
        };
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

#[instrument]
pub(super) fn eval_fn(ctx: &mut Context, name: &str, args: &[Expr]) -> EvalResult {
    if let Some(function) = FUNCTIONS.get(name) {
        return function(ctx, name, args);
    }

    let alias_key = format!("branchless.revsets.alias.{}", name);
    let alias_template: Option<String> = ctx
        .repo
        .get_readonly_config()
        .map_err(EvalError::RepoError)?
        .get(alias_key)
        .map_err(EvalError::OtherError)?;
    if let Some(alias_template) = alias_template {
        let alias_expr = parse(&alias_template).map_err(|err| EvalError::ParseAlias {
            alias: alias_template.clone(),
            source: err,
        })?;
        let arg_map: HashMap<String, Expr> = args
            .iter()
            .enumerate()
            .map(|(i, arg)| (format!("${}", i + 1), arg.clone()))
            .collect();
        let alias_expr = alias_expr.replace_names(&arg_map);
        let commits = eval_inner(ctx, &alias_expr)?;
        return Ok(commits);
    }

    Err(EvalError::UnboundFunction {
        name: name.to_owned(),
        available_names: FUNCTIONS.keys().sorted().copied().collect(),
    })
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

#[instrument]
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

#[instrument]
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

    use lib::core::dag::commit_set_to_vec;
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
        let mut commits: Vec<Commit> = commit_set_to_vec(&result)?
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
            let expr = Expr::FunctionCall(Cow::Borrowed("main"), vec![]);
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
            let expr = Expr::FunctionCall(Cow::Borrowed("public"), vec![]);
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
                Cow::Borrowed("message"),
                vec![Expr::Name(Cow::Borrowed("exact:create test4.txt"))],
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
                vec![Expr::Name(Cow::Borrowed("regex:^create test4.txt$"))],
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

        git.write_file_txt("test1", "test\n")?;
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

        git.write_file_txt("test2", "test\n")?;
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

    #[test]
    fn test_eval_current() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        git.detach_head()?;
        let test1_oid = git.commit_file("test1", 1)?;
        let _test2_oid = git.commit_file("test2", 2)?;
        let test3_oid = git.commit_file("test3", 3)?;
        let test4_oid = git.commit_file("test4", 4)?;

        git.run(&[
            "move",
            "-s",
            &test3_oid.to_string(),
            "-d",
            &test1_oid.to_string(),
        ])?;
        git.run(&["reword", "-m", "test4 has been rewritten twice"])?;

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
            let original_test3_oid = &test3_oid.to_string();
            let original_test4_oid = &test4_oid.to_string();

            let expr = Expr::FunctionCall(
                Cow::Borrowed("union"),
                vec![
                    Expr::Name(Cow::Borrowed(original_test3_oid)),
                    Expr::Name(Cow::Borrowed(original_test4_oid)),
                ],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [],
            )
            "###);

            let expr = Expr::FunctionCall(
                Cow::Borrowed("current"),
                vec![Expr::FunctionCall(
                    Cow::Borrowed("union"),
                    vec![
                        Expr::Name(Cow::Borrowed(original_test3_oid)),
                        Expr::Name(Cow::Borrowed(original_test4_oid)),
                    ],
                )],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Ok(
                [
                    Commit {
                        inner: Commit {
                            id: 4838e49b08954becdd17c0900c1179c2c654c627,
                            summary: "create test3.txt",
                        },
                    },
                    Commit {
                        inner: Commit {
                            id: 619162078182d2c6d80ff604b81e7c2afc3295b7,
                            summary: "test4 has been rewritten twice",
                        },
                    },
                ],
            )
            "###);
        }
        Ok(())
    }

    #[test]
    fn test_eval_aliases() -> eyre::Result<()> {
        let git = make_git()?;
        git.init_repo()?;

        git.detach_head()?;
        let _test1_oid = git.commit_file("test1", 1)?;
        let _test2_oid = git.commit_file("test2", 2)?;
        let _test3_oid = git.commit_file("test3", 3)?;

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
            git.run(&[
                "config",
                "branchless.revsets.alias.simpleAlias",
                "roots($1)",
            ])?;

            let expr = Expr::FunctionCall(
                Cow::Borrowed("simpleAlias"),
                vec![Expr::FunctionCall(Cow::Borrowed("stack"), vec![])],
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
                ],
            )
            "###);
        }

        {
            git.run(&[
                "config",
                "branchless.revsets.alias.complexAlias",
                "children($1) & parents($1)",
            ])?;

            let expr = Expr::FunctionCall(
                Cow::Borrowed("complexAlias"),
                vec![Expr::FunctionCall(Cow::Borrowed("stack"), vec![])],
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
                ],
            )
            "###);
        }

        {
            git.run(&["config", "branchless.revsets.alias.parseError", "foo("])?;

            let (_stdout, _stderr) = git.run_with_options(
                &["query", "parseError()"],
                &GitRunOptions {
                    expected_exit_code: 1,
                    ..Default::default()
                },
            )?;
            insta::assert_snapshot!(_stderr, @r###"
            Evaluation error for expression 'parseError()': failed to parse alias expression 'foo('
            parse error: Unrecognized EOF found at 4
            Expected one of "(", ")", "..", ":", "::", a commit/branch/tag or a string literal
            "###);
        }

        {
            // Check for macro hygiene: arguments from outer nested aliases
            // should not be available inside inner aliases.
            //
            // 1. User input: `outerAlias(a, b)` (2 arguments provided)
            // 2. Expands to: `innerAlias(a)` (only uses 1 arg)
            // 3. Expands to: `builtin(a, $2)` (uses 2 args)
            //
            // In this case, there is no $2 available for step 3, so we want to
            // ensure that $2 is not resolved from step 1 and that it instead
            // fails.
            git.run(&[
                "config",
                "branchless.revsets.alias.outerAlias",
                "innerAlias($1)",
            ])?;

            git.run(&[
                "config",
                "branchless.revsets.alias.innerAlias",
                "intersection($1, $2)",
            ])?;

            let expr = Expr::FunctionCall(
                Cow::Borrowed("outerAlias"),
                vec![
                    Expr::FunctionCall(Cow::Borrowed("stack"), vec![]),
                    Expr::FunctionCall(Cow::Borrowed("nonsense"), vec![]),
                ],
            );
            insta::assert_debug_snapshot!(eval_and_sort(&effects, &repo, &mut dag, &expr), @r###"
            Err(
                UnboundName {
                    name: "$2",
                },
            )
            "###);
        }

        Ok(())
    }
}
