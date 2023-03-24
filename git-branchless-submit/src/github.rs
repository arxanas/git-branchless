//! GitHub backend for submitting patch stacks.

use std::collections::HashMap;

use lib::core::dag::CommitSet;
use lib::core::dag::Dag;
use lib::core::effects::Effects;
use lib::core::eventlog::EventLogDb;
use lib::git::GitRunInfo;
use lib::git::{NonZeroOid, Repo};

use lib::util::EyreExitOr;

use crate::{CommitStatus, CreateStatus, Forge, SubmitOptions};

/// The [GitHub](https://en.wikipedia.org/wiki/GitHub) code hosting platform.
/// This forge integrates specifically with the `gh` command-line utility.
#[allow(missing_docs)]
#[derive(Debug)]
pub struct GithubForge<'a> {
    pub effects: &'a Effects,
    pub git_run_info: &'a GitRunInfo,
    pub repo: &'a Repo,
    pub event_log_db: &'a EventLogDb<'a>,
    pub dag: &'a Dag,
}

impl Forge for GithubForge<'_> {
    fn query_status(
        &mut self,
        _commit_set: CommitSet,
    ) -> EyreExitOr<HashMap<NonZeroOid, CommitStatus>> {
        unimplemented!("stub")
    }

    fn create(
        &mut self,
        _commits: HashMap<NonZeroOid, CommitStatus>,
        _options: &SubmitOptions,
    ) -> EyreExitOr<HashMap<NonZeroOid, CreateStatus>> {
        unimplemented!("stub")
    }

    fn update(
        &mut self,
        _commits: HashMap<NonZeroOid, CommitStatus>,
        _options: &SubmitOptions,
    ) -> EyreExitOr<()> {
        unimplemented!("stub")
    }
}
