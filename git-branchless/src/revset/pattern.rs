use std::{
    convert::TryFrom,
    sync::{Arc, Mutex},
};

use lib::{
    core::{
        dag::{CommitSet, CommitVertex},
        effects::{Effects, OperationType},
        rewrite::RepoResource,
    },
    git::{Commit, NonZeroOid, Repo},
};
use rayon::prelude::{ParallelBridge, ParallelIterator};
use regex::Regex;
use thiserror::Error;

use crate::revset::eval::make_dag_backend_error;

use super::eval::{Context, EvalError};

pub(super) enum Pattern {
    Exact(String),
    Substring(String),
    Glob(glob::Pattern),
    Regex(regex::Regex),
}

#[derive(Debug, Error)]
pub enum PatternError {
    #[error("failed to compile glob: {0}")]
    CompileGlob(#[from] glob::PatternError),

    #[error("failed to compile regex: {0}")]
    CompileRegex(#[from] regex::Error),

    #[error(transparent)]
    Eval(#[from] Box<EvalError>),

    #[error("failed to query repo: {0}")]
    Repo(eyre::Error),

    #[error("failed to construct matcher object: {0}")]
    ConstructMatcher(eyre::Error),
}

impl Pattern {
    pub fn matches_text(&self, subject: &str) -> bool {
        match self {
            Pattern::Exact(pattern) => pattern == subject,
            Pattern::Substring(pattern) => subject.contains(pattern),
            Pattern::Glob(pattern) => pattern.matches(subject),
            Pattern::Regex(pattern) => pattern.is_match(subject),
        }
    }

    pub fn new(pattern: &str) -> Result<Self, PatternError> {
        if let Some(pattern) = pattern.strip_prefix("exact:") {
            return Ok(Pattern::Exact(pattern.to_owned()));
        }
        if let Some(pattern) = pattern.strip_prefix("substring:") {
            return Ok(Pattern::Substring(pattern.to_owned()));
        }
        if let Some(pattern) = pattern.strip_prefix("substr:") {
            return Ok(Pattern::Substring(pattern.to_owned()));
        }
        if let Some(pattern) = pattern.strip_prefix("glob:") {
            let pattern = glob::Pattern::new(pattern)?;
            return Ok(Pattern::Glob(pattern));
        }
        if let Some(pattern) = pattern.strip_prefix("regex:") {
            let pattern = Regex::new(pattern)?;
            return Ok(Pattern::Regex(pattern));
        }
        Ok(Pattern::Substring(pattern.to_owned()))
    }
}

pub(super) trait PatternMatcher: Sync + Send {
    fn get_description(&self) -> &str;
    fn matches_commit(&self, repo: &Repo, commit: &Commit) -> Result<bool, PatternError>;
}

pub(super) fn make_pattern_matcher_set(
    ctx: &mut Context,
    repo: &Repo,
    matcher: Box<dyn PatternMatcher>,
) -> Result<CommitSet, PatternError> {
    struct Wrapped {
        effects: Effects,
        repo: Repo,
        active_commits: CommitSet,
        matcher: Box<dyn PatternMatcher>,
    }
    let wrapped = Arc::new(Mutex::new(Wrapped {
        effects: ctx.effects.clone(),
        repo: repo.try_clone().map_err(PatternError::Repo)?,
        active_commits: ctx.query_active_commits().map_err(Box::new)?.clone(),
        matcher,
    }));

    Ok(CommitSet::from_evaluate_contains(
        // Function to evaluate entire set.
        {
            let wrapped = Arc::clone(&wrapped);
            move || {
                let wrapped = wrapped.lock().unwrap();
                let Wrapped {
                    effects,
                    repo,
                    active_commits,
                    matcher,
                } = &*wrapped;

                let (effects, progress) = effects.start_operation(OperationType::EvaluateRevset(
                    Arc::new(matcher.get_description().to_owned()),
                ));
                let _effects = effects;

                let len = active_commits.count()?;
                progress.notify_progress(0, len);

                let repo_pool = RepoResource::new_pool(repo).map_err(make_dag_backend_error)?;
                let commit_oids = active_commits.iter()?;
                let result = commit_oids
                    .par_bridge()
                    .try_fold(
                        Vec::new,
                        |mut acc, commit_oid| -> Result<Vec<_>, eden_dag::Error> {
                            let commit_oid: CommitVertex = commit_oid?;
                            let commit_oid =
                                NonZeroOid::try_from(commit_oid).map_err(make_dag_backend_error)?;
                            let repo = repo_pool.try_create().map_err(make_dag_backend_error)?;
                            let commit = repo
                                .find_commit_or_fail(commit_oid)
                                .map_err(make_dag_backend_error)?;
                            if matcher
                                .matches_commit(&*repo, &commit)
                                .map_err(make_dag_backend_error)?
                            {
                                acc.push(commit_oid);
                            }
                            progress.notify_progress_inc(1);
                            Ok(acc)
                        },
                    )
                    .try_reduce(Vec::new, |mut acc, item| {
                        acc.extend(item);
                        Ok(acc)
                    })?;
                let result: CommitSet = result.into_iter().collect();
                Ok(result)
            }
        },
        // Fast path to check for containment.
        move |_self, vertex| {
            let wrapped = wrapped.lock().unwrap();
            let Wrapped {
                effects,
                repo,
                active_commits,
                matcher,
            } = &*wrapped;
            let _effects = effects;

            if !active_commits.contains(vertex)? {
                return Ok(false);
            }

            let oid = NonZeroOid::try_from(vertex.clone()).map_err(make_dag_backend_error)?;
            let commit = repo
                .find_commit_or_fail(oid)
                .map_err(make_dag_backend_error)?;
            let result = matcher
                .matches_commit(&*repo, &commit)
                .map_err(make_dag_backend_error)?;
            Ok(result)
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern() -> eyre::Result<()> {
        assert!(Pattern::new("bar")?.matches_text("foo bar baz"));
        assert!(Pattern::new("substring:bar")?.matches_text("foo bar baz"));
        assert!(!Pattern::new("exact:bar")?.matches_text("foo bar baz"));
        assert!(Pattern::new("exact:foo bar baz")?.matches_text("foo bar baz"));

        assert!(!Pattern::new("glob:b*r")?.matches_text("foo bar baz"));
        assert!(Pattern::new("glob:*b*r*")?.matches_text("foo bar baz"));
        assert!(Pattern::new("glob:a**").is_err());

        assert!(Pattern::new("regex:.*b.*r.*")?.matches_text("foo bar baz"));
        assert!(!Pattern::new("regex:^b.*r$")?.matches_text("foo bar baz"));
        assert!(Pattern::new("regex:[").is_err());

        Ok(())
    }
}
