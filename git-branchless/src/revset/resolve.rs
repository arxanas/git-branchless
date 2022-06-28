use std::fmt::Write;

use lib::core::dag::{CommitSet, Dag};
use lib::core::effects::Effects;
use lib::git::Repo;
use tracing::instrument;

use crate::opts::Revset;

use super::eval::EvalError;
use super::parser::ParseError;
use super::{eval, parse};

/// The result of attempting to resolve commits.
#[allow(clippy::enum_variant_names)]
#[derive(Debug)]
pub enum ResolveError {
    ParseError { expr: String, source: ParseError },
    EvalError { expr: String, source: EvalError },
    DagError { source: eden_dag::Error },
    OtherError { source: eyre::Error },
}

impl ResolveError {
    pub fn describe(self, effects: &Effects) -> eyre::Result<()> {
        match self {
            ResolveError::ParseError { expr, source } => {
                writeln!(
                    effects.get_error_stream(),
                    "Parse error for expression '{}': {}",
                    expr,
                    source
                )?;
                Ok(())
            }
            ResolveError::EvalError { expr, source } => {
                writeln!(
                    effects.get_error_stream(),
                    "Evaluation error for expression '{}': {}",
                    expr,
                    source
                )?;
                Ok(())
            }
            ResolveError::DagError { source } => Err(source.into()),
            ResolveError::OtherError { source } => Err(source),
        }
    }
}

/// Parse strings which refer to commits, such as:
///
/// - Full OIDs.
/// - Short OIDs.
/// - Reference names.
#[instrument]
pub fn resolve_commits(
    effects: &Effects,
    repo: &Repo,
    dag: &mut Dag,
    revsets: Vec<Revset>,
) -> Result<Vec<CommitSet>, ResolveError> {
    let mut commit_sets = Vec::new();
    for Revset(revset) in revsets {
        // Handle syntax that's supported by Git, but which we haven't
        // implemented in the revset language.
        if let Ok(Some(commit)) = repo.revparse_single_commit(&revset) {
            let commit_set = CommitSet::from(commit.get_oid());
            dag.sync_from_oids(effects, repo, CommitSet::empty(), commit_set.clone())
                .map_err(|err| ResolveError::OtherError { source: err })?;
            commit_sets.push(commit_set);
            continue;
        }

        let expr = parse(&revset).map_err(|err| ResolveError::ParseError {
            expr: revset.clone(),
            source: err,
        })?;
        let commits = eval(effects, repo, dag, &expr).map_err(|err| ResolveError::EvalError {
            expr: revset.clone(),
            source: err,
        })?;

        // Note that commits which are identified by `revparse_single_commit`
        // above will still be returned in the result. This ends up resulting in
        // fairly intuitive behavior: addressing a commit directly by hash (or
        // simple revset expression, such as `HEAD^`) will result in it being
        // returned, but not when using a revset expression like
        // `descendants(@)`.
        let commits = commits.difference(&dag.obsolete_commits);

        commit_sets.push(commits);
    }
    Ok(commit_sets)
}
