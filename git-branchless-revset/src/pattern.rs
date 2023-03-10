use std::convert::TryFrom;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use chrono::{Local, NaiveDateTime};
use chrono_english::{parse_date_string, parse_duration, DateError, Dialect, Interval};
use chronoutil::RelativeDuration;
use eden_dag::nameset::hints::Hints;
use futures::StreamExt;
use lib::core::dag::{CommitSet, CommitVertex};
use lib::core::effects::{Effects, OperationType};
use lib::core::rewrite::RepoResource;
use lib::git::{Commit, NonZeroOid, Repo, RepoError, Time};
use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use regex::Regex;
use thiserror::Error;

use crate::eval::make_dag_backend_error;
use crate::eval::{Context, EvalError};

pub(super) enum Pattern {
    Exact(String),
    Substring(String),
    Glob(glob::Pattern),
    Regex(regex::Regex),
    Before(NaiveDateTime),
    After(NaiveDateTime),
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
    Repo(#[source] RepoError),

    #[error("failed to construct matcher object: {0}")]
    ConstructMatcher(#[source] eyre::Error),

    #[error("failed to parse date: {0}")]
    Date(#[from] DateError),
}

impl Pattern {
    pub fn matches_text(&self, subject: &str) -> bool {
        let subject = subject.strip_suffix('\n').unwrap_or(subject);
        match self {
            Pattern::Exact(pattern) => pattern == subject,
            Pattern::Substring(pattern) => subject.contains(pattern),
            Pattern::Glob(pattern) => pattern.matches(subject),
            Pattern::Regex(pattern) => pattern.is_match(subject),
            Pattern::Before(_) | Pattern::After(_) => false,
        }
    }

    pub fn matches_date(&self, time: &Time) -> bool {
        match self {
            Pattern::Exact(_) | Pattern::Substring(_) | Pattern::Glob(_) | Pattern::Regex(_) => {
                false
            }
            Pattern::Before(date) => match time.to_naive_date_time() {
                Some(time) => &time <= date,
                None => false,
            },
            Pattern::After(date) => match time.to_naive_date_time() {
                Some(time) => &time >= date,
                None => false,
            },
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

        fn parse_date(pattern: &str) -> Result<NaiveDateTime, PatternError> {
            if let Ok(date) = parse_date_string(pattern, Local::now(), Dialect::Us) {
                return Ok(date.naive_local());
            }
            if let Ok(interval) = parse_duration(pattern) {
                let delta = match interval {
                    Interval::Seconds(seconds) => RelativeDuration::seconds(seconds.into()),
                    Interval::Days(days) => RelativeDuration::days(days.into()),
                    Interval::Months(months) => RelativeDuration::months(months),
                };
                let date = Local::now().naive_local() + delta;
                return Ok(date);
            }
            Err(PatternError::ConstructMatcher(eyre::eyre!(
                "cannot parse date: {pattern}"
            )))
        }

        if let Some(pattern) = pattern.strip_prefix("before:") {
            let date = parse_date(pattern)?;
            return Ok(Pattern::Before(date));
        }
        if let Some(pattern) = pattern.strip_prefix("after:") {
            let date = parse_date(pattern)?;
            return Ok(Pattern::After(date));
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
    struct MatcherNameSetQuery {
        effects: Effects,
        matcher: Box<dyn PatternMatcher>,
        repo: Arc<Mutex<Repo>>,
        visible_commits: CommitSet,
    }

    impl MatcherNameSetQuery {
        async fn evaluate(&self) -> eden_dag::Result<CommitSet> {
            let (effects, progress) =
                self.effects
                    .start_operation(OperationType::EvaluateRevset(Arc::new(
                        self.matcher.get_description().to_owned(),
                    )));
            let _effects = effects;

            let len = self.visible_commits.count().await?;
            progress.notify_progress(0, len);

            let stream = self.visible_commits.iter().await?;
            let commit_oids = stream.collect::<Vec<_>>().await;
            let repo = self.repo.lock().unwrap();
            let repo_pool = RepoResource::new_pool(&repo).map_err(make_dag_backend_error)?;
            let result = commit_oids
                .into_par_iter()
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
                        if self
                            .matcher
                            .matches_commit(&repo, &commit)
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

        async fn contains(&self, name: &CommitVertex) -> eden_dag::Result<bool> {
            if !self.visible_commits.contains(name).await? {
                return Ok(false);
            }

            let oid = NonZeroOid::try_from(name.clone()).map_err(make_dag_backend_error)?;
            let repo = self.repo.lock().unwrap();
            let commit = repo
                .find_commit_or_fail(oid)
                .map_err(make_dag_backend_error)?;
            let result = self
                .matcher
                .matches_commit(&repo, &commit)
                .map_err(make_dag_backend_error)?;
            Ok(result)
        }
    }

    let repo = repo.try_clone().map_err(PatternError::Repo)?;
    let visible_commits = ctx
        .dag
        .query_visible_commits_slow()
        .map_err(EvalError::OtherError)
        .map_err(Box::new)?
        .clone();

    let matcher = Arc::new(MatcherNameSetQuery {
        effects: ctx.effects.clone(),
        matcher,
        repo: Arc::new(Mutex::new(repo)),
        visible_commits,
    });
    Ok(CommitSet::from_async_evaluate_contains(
        {
            let matcher = Arc::clone(&matcher);
            Box::new(move || {
                let matcher = Arc::clone(&matcher);
                Box::pin(async move { matcher.evaluate().await })
            })
        },
        {
            let matcher = Arc::clone(&matcher);
            Box::new(move |_self, name| {
                let matcher = Arc::clone(&matcher);
                Box::pin(async move { matcher.contains(name).await })
            })
        },
        Hints::default(),
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
