use std::collections::{HashSet, VecDeque};
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::{Arc, Condvar, Mutex};

use crossbeam::channel::Sender;
use lib::core::effects::ProgressHandle;
use tracing::{debug, warn};

pub(crate) type WorkerId = usize;

pub trait Job: Clone + Debug + Eq + Hash {}
impl<T: Clone + Debug + Eq + Hash> Job for T {}

#[derive(Debug)]
pub(crate) enum JobResult<J: Job, Output> {
    Done(J, Output),
    Error(WorkerId, J, String),
}

#[derive(Debug)]
struct WorkQueueState<J: Job> {
    jobs: VecDeque<J>,
    accepted_jobs: HashSet<J>,
    is_active: bool,
}

impl<J: Job> Default for WorkQueueState<J> {
    fn default() -> Self {
        Self {
            jobs: Default::default(),
            accepted_jobs: Default::default(),
            is_active: true,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct WorkQueue<J: Job> {
    state: Arc<Mutex<WorkQueueState<J>>>,
    cond_var: Arc<Condvar>,
}

impl<J: Job> WorkQueue<J> {
    pub fn new() -> Self {
        Self {
            state: Default::default(),
            cond_var: Default::default(),
        }
    }

    pub fn set(&self, jobs: Vec<J>) {
        let mut state = self.state.lock().unwrap();
        state.jobs = jobs
            .into_iter()
            .filter(|job| !state.accepted_jobs.contains(job))
            .collect();
        self.cond_var.notify_all();
    }

    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.jobs.clear();
        state.is_active = false;
        self.cond_var.notify_all();
    }

    pub fn pop_blocking(&self) -> Option<J> {
        enum WakeupCond {
            Inactive,
            NewJob,
        }
        fn wakeup_cond<J: Job>(state: &WorkQueueState<J>) -> Option<WakeupCond> {
            if !state.is_active {
                Some(WakeupCond::Inactive)
            } else if !state.jobs.is_empty() {
                Some(WakeupCond::NewJob)
            } else {
                None
            }
        }

        let mut state = self.state.lock().unwrap();
        loop {
            match wakeup_cond(&state) {
                Some(WakeupCond::Inactive) => break None,
                Some(WakeupCond::NewJob) => {
                    let job = state
                        .jobs
                        .pop_front()
                        .expect("Condition variable should have ensured that jobs is non-empty");
                    if !state.accepted_jobs.insert(job.clone()) {
                        warn!(?job, "Job was already accepted");
                    }
                    break Some(job);
                }
                None => {
                    state = self
                        .cond_var
                        .wait_while(state, |state| wakeup_cond(state).is_none())
                        .unwrap();
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
        debug!(
            ?worker_id,
            ?job,
            ?should_terminate,
            "Worker finished sending Done job result"
        );
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
