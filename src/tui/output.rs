use std::collections::HashMap;
use std::convert::TryInto;
use std::fmt::Write;
use std::io::{stderr, stdout, Write as WriteIo};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use itertools::Itertools;
use lazy_static::lazy_static;
use tracing::warn;

use crate::core::formatting::Glyphs;

#[allow(missing_docs)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationType {
    BuildRebasePlan,
    CalculateDiff,
    CalculatePatchId,
    CheckForCycles,
    DetectDuplicateCommits,
    FindPathToMergeBase,
    GetMergeBase,
    GetUpstreamPatchIds,
    MakeGraph,
    WalkCommits,
}

impl ToString for OperationType {
    fn to_string(&self) -> String {
        let s = match self {
            OperationType::BuildRebasePlan => "Building rebase plan",
            OperationType::CheckForCycles => "Checking for cycles",
            OperationType::CalculateDiff => "Computing diff",
            OperationType::CalculatePatchId => "Hashing commit contents",
            OperationType::DetectDuplicateCommits => "Checking for duplicate commits",
            OperationType::FindPathToMergeBase => "Finding path to merge-base",
            OperationType::GetMergeBase => "Calculating merge-bases",
            OperationType::GetUpstreamPatchIds => "Enumerating patch IDs",
            OperationType::MakeGraph => "Examining local history",
            OperationType::WalkCommits => "Walking commits",
        };
        s.to_string()
    }
}

#[derive(Clone)]
enum OutputDest {
    Stdout,
    SuppressForTest,
    BufferForTest(Arc<Mutex<Vec<u8>>>),
}

struct OperationState {
    operation_type: OperationType,
    progress_bar: ProgressBar,
    has_meter: bool,
    start_times: Vec<Instant>,
    elapsed_duration: Duration,
}

impl OperationState {
    pub fn set_progress(&mut self, current: usize, total: usize) {
        self.has_meter = true;
        self.progress_bar.set_position(current.try_into().unwrap());
        self.progress_bar.set_length(total.try_into().unwrap());
    }

    pub fn inc_progress(&mut self, increment: usize) {
        self.progress_bar.inc(increment.try_into().unwrap());
    }

    pub fn tick(&self) {
        lazy_static! {
            static ref CHECKMARK: String = console::style("✓").green().to_string();
            static ref IN_PROGRESS_SPINNER_STYLE: ProgressStyle =
                ProgressStyle::default_spinner().template("{prefix}{spinner} {msg}");
            static ref IN_PROGRESS_BAR_STYLE: ProgressStyle =
                ProgressStyle::default_bar().template("{prefix}{spinner} {msg} {bar} {pos}/{len}");
            static ref FINISHED_PROGRESS_STYLE: ProgressStyle = IN_PROGRESS_SPINNER_STYLE
                .clone()
                // Requires at least two tick values, so just pass the same one twice.
                .tick_strings(&[&CHECKMARK, &CHECKMARK]);
        }

        let elapsed_duration = match self.start_times.iter().min() {
            None => self.elapsed_duration,
            Some(start_time) => {
                let additional_duration = Instant::now().saturating_duration_since(*start_time);
                self.elapsed_duration + additional_duration
            }
        };

        self.progress_bar.set_message(format!(
            "{} ({:.1}s)",
            self.operation_type.to_string(),
            elapsed_duration.as_secs_f64(),
        ));
        self.progress_bar
            .set_style(match (self.start_times.as_slice(), self.has_meter) {
                ([], _) => FINISHED_PROGRESS_STYLE.clone(),
                ([..], false) => IN_PROGRESS_SPINNER_STYLE.clone(),
                ([..], true) => IN_PROGRESS_BAR_STYLE.clone(),
            });
        self.progress_bar.tick();
    }
}

/// Wrapper around output. Also manages progress indicators.
#[derive(Clone)]
pub struct Output {
    glyphs: Glyphs,
    dest: OutputDest,
    multi_progress: Arc<MultiProgress>,
    nesting_level: usize,
    operation_states: Arc<RwLock<HashMap<OperationType, OperationState>>>,
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "<Output fancy={}>",
            self.glyphs.should_write_ansi_escape_codes
        )
    }
}

fn spawn_progress_updater_thread(
    multi_progress: &Arc<MultiProgress>,
    operation_states: &Arc<RwLock<HashMap<OperationType, OperationState>>>,
) {
    multi_progress.set_draw_target(ProgressDrawTarget::hidden());
    let multi_progress = Arc::downgrade(multi_progress);
    let operation_states = Arc::downgrade(operation_states);
    thread::spawn(move || {
        // Don't start displaying progress immediately, since if the operation
        // finishes quickly, then it will flicker annoyingly.
        thread::sleep(Duration::from_millis(250));
        if let Some(multi_progress) = multi_progress.upgrade() {
            multi_progress.set_draw_target(ProgressDrawTarget::stderr());
        }

        loop {
            // Drop the `Arc` after this block, before the sleep, to make sure
            // that progress bars aren't kept alive longer than they should be.
            match operation_states.upgrade() {
                None => return,
                Some(operation_states) => {
                    let operation_states = operation_states.read().unwrap();
                    for operation_state in operation_states.values() {
                        operation_state.tick();
                    }
                }
            }

            thread::sleep(Duration::from_millis(100));
        }
    });
}

impl Output {
    /// Constructor. Writes to stdout.
    pub fn new(glyphs: Glyphs) -> Self {
        let multi_progress = Default::default();
        let operation_states = Default::default();
        spawn_progress_updater_thread(&multi_progress, &operation_states);
        Output {
            glyphs,
            dest: OutputDest::Stdout,
            multi_progress,
            nesting_level: Default::default(),
            operation_states,
        }
    }

    /// Constructor. Suppresses all output.
    pub fn new_suppress_for_test(glyphs: Glyphs) -> Self {
        Output {
            glyphs,
            dest: OutputDest::SuppressForTest,
            multi_progress: Default::default(),
            nesting_level: Default::default(),
            operation_states: Default::default(),
        }
    }

    /// Constructor. Writes to the provided buffer.
    pub fn new_from_buffer_for_test(glyphs: Glyphs, buffer: &Arc<Mutex<Vec<u8>>>) -> Self {
        Output {
            glyphs,
            dest: OutputDest::BufferForTest(Arc::clone(buffer)),
            multi_progress: Default::default(),
            nesting_level: Default::default(),
            operation_states: Default::default(),
        }
    }

    /// Start reporting progress for the specified operation type.
    ///
    /// A progress spinner is shown until the returned `ProgressHandle` is
    /// dropped, at which point the spinner transitions to a "complete" message.
    ///
    /// Progress spinners are nested hierarchically. If this function is called
    /// while another `ProgressHandle` is still alive, then the returned
    /// `ProgressHandle` will be a child of the first handle.
    ///
    /// None of the progress indicators are cleared from the screen until *all*
    /// of the operations have completed. Furthermore, their internal timer is
    /// not reset while they are still on the screen.
    ///
    /// If you finish and then start the same operation type again while it is
    /// still displayed, then it will transition back into the "progress" state,
    /// and its displayed duration will start increasing from its previous
    /// value, rather than from zero. The practical implication is that you can
    /// see the aggregate time it took to carry out sibling operations, i.e. the
    /// same operation called multiple times in a loop.
    pub fn start_operation(&self, operation_type: OperationType) -> (Output, ProgressHandle) {
        let now = Instant::now();
        let mut operation_states = self.operation_states.write().unwrap();

        let mut nesting_level = self.nesting_level;
        let operation_state = operation_states.entry(operation_type).or_insert_with(|| {
            let progress_bar = self.multi_progress.add(ProgressBar::new_spinner());
            nesting_level += 1;
            progress_bar.set_prefix("  ".repeat(nesting_level));
            let operation_state = OperationState {
                operation_type,
                progress_bar,
                start_times: vec![now],
                has_meter: false,
                elapsed_duration: Default::default(),
            };
            operation_state.tick();
            operation_state
        });
        operation_state.start_times.push(now);

        let output = Self {
            nesting_level,
            ..self.clone()
        };
        let progress = ProgressHandle {
            output: self,
            operation_type,
        };
        (output, progress)
    }

    fn on_notify_progress(&self, operation_type: OperationType, current: usize, total: usize) {
        let mut operation_states = self.operation_states.write().unwrap();
        let operation_state = match operation_states.get_mut(&operation_type) {
            Some(operation_state) => operation_state,
            None => return,
        };

        operation_state.set_progress(current, total);
        operation_state.tick();
    }

    fn on_notify_progress_inc(&self, operation_type: OperationType, increment: usize) {
        let mut operation_states = self.operation_states.write().unwrap();
        let operation_state = match operation_states.get_mut(&operation_type) {
            Some(operation_state) => operation_state,
            None => return,
        };

        operation_state.inc_progress(increment);
        operation_state.tick();
    }

    fn on_drop_progress_handle(&self, operation_type: OperationType) {
        let now = Instant::now();
        let mut operation_states = self.operation_states.write().unwrap();

        let operation_state = match operation_states.get_mut(&operation_type) {
            Some(operation_state) => operation_state,
            None => {
                warn!("Progress operation not started");
                return;
            }
        };

        let previous_start_time = match operation_state
            .start_times
            .iter()
            // Remove a maximum element each time. This means that the last
            // element removed will be the minimum of all elements seen over
            // time.
            .position_max_by_key(|x| *x)
        {
            Some(start_time_index) => operation_state.start_times.remove(start_time_index),
            None => {
                warn!("Progress operation ended without matching start call");
                return;
            }
        };

        // In the event of multiple concurrent operations, we only want to add
        // the wall-clock time to the elapsed duration.
        operation_state.elapsed_duration += if operation_state.start_times.is_empty() {
            now.saturating_duration_since(previous_start_time)
        } else {
            Duration::ZERO
        };

        // Reset all elapsed times only if the root-level operation completed.
        if operation_states
            .values()
            .all(|operation_state| operation_state.start_times.is_empty())
        {
            self.multi_progress.clear().unwrap();
            operation_states.clear();
        }
    }

    /// Get the set of glyphs associated with the output.
    pub fn get_glyphs(&self) -> &Glyphs {
        &self.glyphs
    }

    /// Create a stream that can be written to. The output might go to stdout or
    /// be rendered specially in the terminal.
    pub fn get_output_stream(&self) -> OutputStream {
        OutputStream {
            dest: self.dest.clone(),
        }
    }

    /// Create a stream that error output can be written to, rather than regular
    /// output.
    pub fn get_error_stream(&self) -> ErrorStream {
        ErrorStream {
            dest: self.dest.clone(),
        }
    }
}

pub struct OutputStream {
    dest: OutputDest,
}

impl Write for OutputStream {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        match &self.dest {
            OutputDest::Stdout => {
                print!("{}", s);
                stdout().flush().unwrap();
            }

            OutputDest::SuppressForTest => {
                // Do nothing.
            }

            OutputDest::BufferForTest(buffer) => {
                let mut buffer = buffer.lock().unwrap();
                write!(buffer, "{}", s).unwrap();
            }
        }
        Ok(())
    }
}

pub struct ErrorStream {
    dest: OutputDest,
}

impl Write for ErrorStream {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        match &self.dest {
            OutputDest::Stdout => {
                eprint!("{}", s);
                stderr().flush().unwrap();
            }

            OutputDest::SuppressForTest => {
                // Do nothing.
            }

            OutputDest::BufferForTest(_) => {
                // Drop the error output, as the buffer only represents `stdout`.
            }
        }
        Ok(())
    }
}

pub struct ProgressHandle<'output> {
    output: &'output Output,
    operation_type: OperationType,
}

impl Drop for ProgressHandle<'_> {
    fn drop(&mut self) {
        self.output.on_drop_progress_handle(self.operation_type)
    }
}

impl ProgressHandle<'_> {
    pub fn notify_progress(&self, current: usize, total: usize) {
        self.output
            .on_notify_progress(self.operation_type, current, total);
    }

    pub fn notify_progress_inc(&self, increment: usize) {
        self.output
            .on_notify_progress_inc(self.operation_type, increment);
    }
}
