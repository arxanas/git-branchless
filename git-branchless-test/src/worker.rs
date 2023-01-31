use std::collections::VecDeque;
use std::sync::{mpsc::Sender, Arc, Mutex};

use lib::core::effects::ProgressHandle;
use tracing::debug;

use crate::TestOutput;

pub(crate) type WorkerId = usize;
pub(crate) type WorkQueue<Job> = Arc<Mutex<VecDeque<Job>>>;
pub(crate) enum JobResult<Job, Output> {
    Done(Job, Output),
    Error(WorkerId, Job, String),
}

pub(crate) fn worker<Job: Clone + std::fmt::Debug, Context>(
    progress: &ProgressHandle,
    worker_id: WorkerId,
    work_queue: WorkQueue<Job>,
    result_tx: Sender<JobResult<Job, TestOutput>>,
    setup: impl Fn() -> eyre::Result<Context>,
    f: impl Fn(Job, &Context) -> eyre::Result<TestOutput>,
) {
    debug!(?worker_id, "Worker spawned");

    let context = match setup() {
        Ok(context) => context,
        Err(err) => {
            panic!("Worker {worker_id} could not open repository at: {err}");
        }
    };

    let run_job = |job: Job| -> eyre::Result<bool> {
        let test_output = f(job.clone(), &context)?;
        progress.notify_progress_inc(1);

        debug!(?worker_id, ?job, "Worker sending Done job result");
        let should_terminate = result_tx
            .send(JobResult::Done(job.clone(), test_output))
            .is_err();
        debug!(?worker_id, ?job, "Worker finished sending Done job result");
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
        debug!(?worker_id, ?job, "Worker accepted job");
        let job_result = run_job(job.clone());
        debug!(?worker_id, ?job, "Worker finished job");
        match job_result {
            Ok(true) => break,
            Ok(false) => {
                // Continue.
            }
            Err(err) => {
                debug!(?worker_id, ?job, "Worker sending Error job result");
                result_tx
                    .send(JobResult::Error(worker_id, job.clone(), err.to_string()))
                    .ok();
                debug!(?worker_id, ?job, "Worker sending Error job result");
            }
        }
    }
    debug!(?worker_id, "Worker exiting");
}
