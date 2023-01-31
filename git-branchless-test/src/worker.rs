use std::collections::VecDeque;
use std::path::Path;
use std::sync::{mpsc::Sender, Arc, Mutex};

use lib::core::effects::{Effects, OperationType, ProgressHandle};
use lib::core::eventlog::EventTransactionId;
use lib::git::GitRunInfo;
use lib::git::{NonZeroOid, Repo};
use tracing::debug;

use crate::{run_test, ResolvedTestOptions, TestOutput};

pub(crate) type WorkerId = usize;
type Job = (NonZeroOid, OperationType);
type WorkQueue = Arc<Mutex<VecDeque<Job>>>;
pub(crate) enum JobResult {
    Done(NonZeroOid, TestOutput),
    Error(WorkerId, NonZeroOid, String),
}

pub(crate) fn worker(
    effects: &Effects,
    progress: &ProgressHandle,
    shell_path: &Path,
    git_run_info: &GitRunInfo,
    repo_dir: &Path,
    event_tx_id: EventTransactionId,
    options: &ResolvedTestOptions,
    worker_id: WorkerId,
    work_queue: WorkQueue,
    result_tx: Sender<JobResult>,
) {
    debug!(?worker_id, "Worker spawned");

    let repo = match Repo::from_dir(repo_dir) {
        Ok(repo) => repo,
        Err(err) => {
            panic!("Worker {worker_id} could not open repository at: {err}");
        }
    };

    let run_job = |job: Job| -> eyre::Result<bool> {
        let (commit_oid, operation_type) = job;
        let commit = repo.find_commit_or_fail(commit_oid)?;
        let test_output = run_test(
            effects,
            operation_type,
            git_run_info,
            shell_path,
            &repo,
            event_tx_id,
            options,
            worker_id,
            &commit,
        )?;
        progress.notify_progress_inc(1);

        debug!(?worker_id, ?commit_oid, "Worker sending Done job result");
        let should_terminate = result_tx
            .send(JobResult::Done(commit_oid, test_output))
            .is_err();
        debug!(
            ?worker_id,
            ?commit_oid,
            "Worker finished sending Done job result"
        );
        Ok(should_terminate)
    };

    while let Some(job) = {
        debug!(?worker_id, "Locking work queue");
        let mut work_queue = work_queue.lock().unwrap();
        debug!(?worker_id, "Locked work queue");
        let job = work_queue.pop_front();
        // Ensure we don't hold the lock while we process the job.
        drop(work_queue);
        debug!(?worker_id, "Unlocked work queue");
        job
    } {
        let (commit_oid, _) = job;
        debug!(?worker_id, ?commit_oid, "Worker accepted job");
        let job_result = run_job(job);
        debug!(?worker_id, ?commit_oid, "Worker finished job");
        match job_result {
            Ok(true) => break,
            Ok(false) => {
                // Continue.
            }
            Err(err) => {
                debug!(?worker_id, ?commit_oid, "Worker sending Error job result");
                result_tx
                    .send(JobResult::Error(worker_id, commit_oid, err.to_string()))
                    .ok();
                debug!(?worker_id, ?commit_oid, "Worker sending Error job result");
            }
        }
    }
    debug!(?worker_id, "Worker exiting");
}
