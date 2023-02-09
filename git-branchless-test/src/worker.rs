use std::collections::HashSet;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use crossbeam::channel::{Receiver, RecvError, SendError, Sender, TryRecvError};
use lib::core::effects::ProgressHandle;
use tracing::debug;

pub(crate) type WorkerId = usize;

pub trait Job: Clone + Debug + Eq + Hash {}
impl<T: Clone + Debug + Eq + Hash> Job for T {}

pub(crate) enum JobResult<J: Job, Output> {
    Done(J, Output),
    Error(WorkerId, J, String),
}

#[derive(Clone, Debug)]
pub(crate) struct WorkQueue<J: Job> {
    job_tx: Arc<Mutex<Option<Sender<J>>>>,
    job_rx: Receiver<J>,
    accepted_jobs: Arc<Mutex<HashSet<J>>>,
}

impl<J: Job> WorkQueue<J> {
    pub fn new() -> Self {
        let (job_tx, job_rx) = crossbeam::channel::unbounded();
        Self {
            job_tx: Arc::new(Mutex::new(Some(job_tx))),
            job_rx,
            accepted_jobs: Default::default(),
        }
    }

    pub fn set(&self, jobs: Vec<J>) {
        let job_tx = self.job_tx.lock().unwrap();
        let job_tx = match job_tx.as_ref() {
            Some(job_tx) => job_tx,
            None => {
                debug!(?jobs, "Tried to set jobs when work queue was disconnected");
                return;
            }
        };
        loop {
            match self.job_rx.try_recv() {
                Ok(job) => {
                    debug!(?job, "Cancelling scheduled job");
                }
                Err(TryRecvError::Empty) => {
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    return;
                }
            }
        }
        for job in jobs {
            debug!(?job, "Scheduling job");
            match job_tx.send(job) {
                Ok(()) => {}
                Err(SendError(job)) => {
                    debug!(?job, "Failed to schedule job");
                }
            }
        }
    }

    pub fn close(&self) {
        self.set(Default::default());
        let mut job_tx = self.job_tx.lock().unwrap();
        *job_tx = None;
    }

    pub fn pop_blocking(&self) -> Option<J> {
        loop {
            match self.job_rx.recv() {
                Ok(job) => {
                    let mut accepted_jobs = self.accepted_jobs.lock().unwrap();
                    if accepted_jobs.insert(job.clone()) {
                        break Some(job);
                    } else {
                        debug!(?job, "Skipped already-accepted job");
                    }
                }
                Err(RecvError) => {
                    debug!("Work queue disconnected");
                    break None;
                }
            }
        }
    }
}

pub(crate) fn worker<J: Job, Output, Context>(
    progress: &ProgressHandle,
    worker_id: WorkerId,
    work_queue: WorkQueue<J>,
    result_tx: Sender<JobResult<J, Output>>,
    setup: impl Fn() -> eyre::Result<Context>,
    f: impl Fn(J, &Context) -> eyre::Result<Output>,
) {
    debug!(?worker_id, "Worker spawned");

    let context = match setup() {
        Ok(context) => context,
        Err(err) => {
            panic!("Worker {worker_id} could not open repository at: {err}");
        }
    };

    let run_job = |job: J| -> eyre::Result<bool> {
        let test_output = f(job.clone(), &context)?;
        progress.notify_progress_inc(1);

        debug!(?worker_id, ?job, "Worker sending Done job result");
        let should_terminate = result_tx
            .send(JobResult::Done(job.clone(), test_output))
            .is_err();
        debug!(?worker_id, ?job, "Worker finished sending Done job result");
        Ok(should_terminate)
    };

    while let Some(job) = work_queue.pop_blocking() {
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
