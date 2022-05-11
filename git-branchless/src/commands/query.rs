use std::fmt::Write;

use eden_dag::DagAlgorithm;
use lib::core::dag::{commit_set_to_vec, Dag};
use lib::core::effects::{Effects, OperationType};
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::repo_ext::RepoExt;
use lib::git::{GitRunInfo, Repo};
use lib::util::ExitCode;
use tracing::instrument;

use crate::opts::Revset;
use crate::revset::resolve_commits;

#[instrument]
pub fn query(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    query: Revset,
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

    let commit_oids = {
        let (effects, _progress) = effects.start_operation(OperationType::SortCommits);
        let _effects = effects;

        dag.query().sort(&commit_set)?;
        commit_set_to_vec(&commit_set)?
    };
    for commit_oid in commit_oids {
        writeln!(effects.get_output_stream(), "{}", commit_oid)?;
    }

    Ok(ExitCode(0))
}
