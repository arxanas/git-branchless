use std::fmt::Write;

use lib::core::dag::{sort_commit_set, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::repo_ext::RepoExt;
use lib::git::{GitRunInfo, Repo};
use lib::util::ExitCode;
use tracing::instrument;

use crate::revset::resolve_commits;

#[instrument]
pub fn query(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    query: String,
) -> eyre::Result<ExitCode> {
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let references_snapshot = repo.get_references_snapshot()?;
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let commit_set = match resolve_commits(effects, &repo, &mut dag, vec![query]) {
        Ok(commit_sets) => commit_sets[0].clone(),
        Err(err) => {
            err.describe(effects)?;
            return Ok(ExitCode(1));
        }
    };

    let commits = sort_commit_set(&repo, &dag, &commit_set)?;
    for commit in commits {
        writeln!(effects.get_output_stream(), "{}", commit.get_oid())?;
    }

    Ok(ExitCode(0))
}
