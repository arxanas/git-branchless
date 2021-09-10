use std::collections::HashMap;
use std::convert::TryInto;
use std::fmt::Write;
use std::io::{stderr, stdout, Stderr, Stdout, Write as WriteIo};
use std::mem::take;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use itertools::Itertools;
use lazy_static::lazy_static;
use tracing::warn;

use crate::core::formatting::Glyphs;

#[allow(missing_docs)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationType {
    BuildRebasePlan,
    CalculateDiff,
    CalculatePatchId,
    CheckForCycles,
    DetectDuplicateCommits,
    FilterByTouchedPaths,
    FilterCommits,
    FindPathToMergeBase,
    GetMergeBase,
    GetTouchedPaths,
    GetUpstreamPatchIds,
    InitializeRebase,
    MakeGraph,
    ProcessEvents,
    RunGitCommand(Arc<String>),
    UpdateCommitGraph,
    WalkCommits,
}

impl ToString for OperationType {
    fn to_string(&self) -> String {
        let s = match self {
            OperationType::BuildRebasePlan => "Building rebase plan",
            OperationType::CalculateDiff => "Computing diffs",
            OperationType::CalculatePatchId => "Hashing commit contents",
            OperationType::CheckForCycles => "Checking for cycles",
            OperationType::DetectDuplicateCommits => "Checking for duplicate commits",
            OperationType::FilterByTouchedPaths => "Filtering upstream commits by touched paths",
            OperationType::FilterCommits => "Filtering commits",
            OperationType::FindPathToMergeBase => "Finding path to merge-base",
            OperationType::GetMergeBase => "Calculating merge-bases",
            OperationType::GetTouchedPaths => "Getting touched paths",
            OperationType::GetUpstreamPatchIds => "Enumerating patch IDs",
            OperationType::InitializeRebase => "Initializing rebase",
            OperationType::MakeGraph => "Examining local history",
            OperationType::ProcessEvents => "Processing events",
            OperationType::RunGitCommand(command) => {
                return format!("Running Git command: {}", &command)
            }
            OperationType::UpdateCommitGraph => "Updating commit graph",
            OperationType::WalkCommits => "Walking commits",
        };
        s.to_string()
    }
}

#[derive(Clone, Debug)]
enum OutputDest {
    Stdout,
    Suppress,
    BufferForTest(Arc<Mutex<Vec<u8>>>),
}

#[derive(Debug)]
struct OperationState {
    operation_type: OperationType,
    progress_bar: ProgressBar,
    is_visible: bool,
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
                ProgressStyle::default_spinner().template("{prefix}{spinner} {wide_msg}");
            static ref IN_PROGRESS_BAR_STYLE: ProgressStyle =
                ProgressStyle::default_bar().template("{prefix}{spinner} {wide_msg} {bar} {pos}/{len}");
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

/// Wrapper around side-effectful operations, such as output and progress
/// indicators.
#[derive(Clone)]
pub struct Effects {
    glyphs: Glyphs,
    dest: OutputDest,
    multi_progress: Arc<MultiProgress>,
    nesting_level: usize,
    operation_states: Arc<RwLock<HashMap<OperationType, OperationState>>>,
}

impl std::fmt::Debug for Effects {
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
            let operation_states = match operation_states.upgrade() {
                None => return,
                Some(operation_states) => operation_states,
            };
            let mut operation_states = operation_states.write().unwrap();

            // Make sure that we set the draw target while we're holding the
            // lock on `operation_states`, so that other consumers don't read an
            // inconsistent value for `is_visible`.
            multi_progress.set_draw_target(ProgressDrawTarget::stderr());
            for operation_state in operation_states.values_mut() {
                operation_state.is_visible = true;
            }
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

impl Effects {
    /// Constructor. Writes to stdout.
    pub fn new(glyphs: Glyphs) -> Self {
        let multi_progress = Default::default();
        let operation_states = Default::default();
        spawn_progress_updater_thread(&multi_progress, &operation_states);
        Effects {
            glyphs,
            dest: OutputDest::Stdout,
            multi_progress,
            nesting_level: Default::default(),
            operation_states,
        }
    }

    /// Constructor. Suppresses all output.
    pub fn new_suppress_for_test(glyphs: Glyphs) -> Self {
        Effects {
            glyphs,
            dest: OutputDest::Suppress,
            multi_progress: Default::default(),
            nesting_level: Default::default(),
            operation_states: Default::default(),
        }
    }

    /// Constructor. Writes to the provided buffer.
    pub fn new_from_buffer_for_test(glyphs: Glyphs, buffer: &Arc<Mutex<Vec<u8>>>) -> Self {
        Effects {
            glyphs,
            dest: OutputDest::BufferForTest(Arc::clone(buffer)),
            multi_progress: Default::default(),
            nesting_level: Default::default(),
            operation_states: Default::default(),
        }
    }

    /// Send output to an appropriate place when using a terminal user interface
    /// (TUI), such as for `git undo`.
    pub fn enable_tui_mode(&self) -> Self {
        let multi_progress = Arc::clone(&self.multi_progress);
        multi_progress.set_draw_target(ProgressDrawTarget::hidden());
        Self {
            dest: OutputDest::Suppress,
            ..self.clone()
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
    pub fn start_operation(&self, operation_type: OperationType) -> (Effects, ProgressHandle) {
        let progress = ProgressHandle {
            effects: self,
            operation_type: operation_type.clone(),
        };
        match self.dest {
            OutputDest::Stdout => {}
            OutputDest::Suppress | OutputDest::BufferForTest(_) => return (self.clone(), progress),
        }

        let now = Instant::now();
        let mut operation_states = self.operation_states.write().unwrap();

        let mut nesting_level = self.nesting_level;
        let operation_state = operation_states
            .entry(operation_type.clone())
            .or_insert_with(|| {
                let progress_bar = self.multi_progress.add(ProgressBar::new_spinner());
                nesting_level += 1;
                progress_bar.set_prefix("  ".repeat(nesting_level));
                let operation_state = OperationState {
                    operation_type,
                    progress_bar,
                    start_times: Vec::new(),
                    is_visible: false,
                    has_meter: false,
                    elapsed_duration: Default::default(),
                };
                operation_state.tick();
                operation_state
            });
        operation_state.start_times.push(now);

        let effects = Self {
            nesting_level,
            ..self.clone()
        };
        (effects, progress)
    }

    fn on_notify_progress(&self, operation_type: OperationType, current: usize, total: usize) {
        let mut operation_states = self.operation_states.write().unwrap();
        let operation_state = match operation_states.get_mut(&operation_type) {
            Some(operation_state) => operation_state,
            None => return,
        };

        operation_state.set_progress(current, total);
    }

    fn on_notify_progress_inc(&self, operation_type: OperationType, increment: usize) {
        let mut operation_states = self.operation_states.write().unwrap();
        let operation_state = match operation_states.get_mut(&operation_type) {
            Some(operation_state) => operation_state,
            None => return,
        };

        operation_state.inc_progress(increment);
    }

    fn on_drop_progress_handle(&self, operation_type: OperationType) {
        match self.dest {
            OutputDest::Stdout => {}
            OutputDest::Suppress | OutputDest::BufferForTest(_) => return,
        }

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
            buffer: Default::default(),
            operation_states: Arc::clone(&self.operation_states),
        }
    }

    /// Create a stream that error output can be written to, rather than regular
    /// output.
    pub fn get_error_stream(&self) -> ErrorStream {
        ErrorStream {
            dest: self.dest.clone(),
            buffer: Default::default(),
            operation_states: Arc::clone(&self.operation_states),
        }
    }
}

trait WriteProgress {
    type Stream: WriteIo;
    fn get_stream() -> Self::Stream;
    fn get_buffer(&mut self) -> &mut String;
    fn get_operation_states(&self) -> Arc<RwLock<HashMap<OperationType, OperationState>>>;
    fn style_output(output: &str) -> String;

    fn flush(&mut self) {
        let operation_states = self.get_operation_states();
        let operation_states = operation_states.read().unwrap();

        // Get an arbitrary progress meter. It turns out that when a
        // `ProgressBar` is included in a `MultiProgress`, it doesn't matter
        // which of them we call `println` on. The output will be printed above
        // the `MultiProgress` regardless.
        match operation_states.values().next() {
            None => {
                // There's no progress meters, so we can write directly to
                // stdout. Note that we don't style output here; instead, we
                // pass through exactly what we got from the caller.
                write!(Self::get_stream(), "{}", take(self.get_buffer())).unwrap();
                Self::get_stream().flush().unwrap();
            }

            Some(_operation_state) if !console::user_attended_stderr() => {
                // The progress meters will be hidden, and any `println`
                // calls on them will be ignored.
                write!(Self::get_stream(), "{}", take(self.get_buffer())).unwrap();
                Self::get_stream().flush().unwrap();
            }

            Some(operation_state) if !operation_state.is_visible => {
                // An operation has started, but we haven't started rendering
                // the progress bars yet, because it hasn't been long enough.
                // We'll write directly to the output stream, but make sure to
                // style the output.
                //
                // Note that we can't use `ProgressBar::is_hidden` to detect if
                // we should enter this case. The `ProgressBar`'s draw target
                // will be set to the parent `MultiProgress`. The parent
                // `MultiProgress` is the object with the hidden output, but
                // it's not possible to get the draw target for the
                // `MultiProgress` at present.
                write!(
                    Self::get_stream(),
                    "{}",
                    Self::style_output(&take(self.get_buffer()))
                )
                .unwrap();
                Self::get_stream().flush().unwrap();
            }

            Some(operation_state) => {
                // Use `Progress::println` to render output above the progress
                // meters. We rely on buffering output because we can only print
                // full lines at a time.
                *self.get_buffer() = {
                    let mut new_buffer = String::new();
                    let lines = self.get_buffer().split_inclusive("\n");
                    for line in lines {
                        match line.strip_suffix('\n') {
                            Some(line) => operation_state
                                .progress_bar
                                .println(Self::style_output(line)),
                            None => {
                                // This should only happen for the last element.
                                new_buffer.push_str(line);
                            }
                        }
                    }
                    new_buffer
                };
            }
        }
    }

    fn drop(&mut self) {
        let buffer = self.get_buffer();
        if !buffer.is_empty() {
            // In practice, we're expecting that every sequence of `write` calls
            // will shortly end with a `write` call that ends with a newline, in
            // such a way that only full lines are written to the output stream.
            // This assumption might be wrong, in which case we should revisit
            // this warning and the behavior on `Drop`.
            warn!(?buffer, "WriteProgress dropped while buffer was not empty");

            // NB: this only flushes completely-written lines, which is not
            // correct. We should flush the entire buffer contents, and possibly
            // force a newline at the end in the case of a progress meter being
            // visible.
            self.flush();
        }
    }
}
pub struct OutputStream {
    dest: OutputDest,
    buffer: String,
    operation_states: Arc<RwLock<HashMap<OperationType, OperationState>>>,
}

impl WriteProgress for OutputStream {
    type Stream = Stdout;

    fn get_stream() -> Self::Stream {
        stdout()
    }

    fn get_buffer(&mut self) -> &mut String {
        &mut self.buffer
    }

    fn get_operation_states(&self) -> Arc<RwLock<HashMap<OperationType, OperationState>>> {
        Arc::clone(&self.operation_states)
    }

    fn style_output(output: &str) -> String {
        console::style(output).dim().to_string()
    }
}

impl Write for OutputStream {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        match &self.dest {
            OutputDest::Stdout => {
                self.buffer.push_str(s);
                self.flush();
            }

            OutputDest::Suppress => {
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

impl Drop for OutputStream {
    fn drop(&mut self) {
        WriteProgress::drop(self)
    }
}

pub struct ErrorStream {
    dest: OutputDest,
    buffer: String,
    operation_states: Arc<RwLock<HashMap<OperationType, OperationState>>>,
}

impl WriteProgress for ErrorStream {
    type Stream = Stderr;

    fn get_stream() -> Self::Stream {
        stderr()
    }

    fn get_buffer(&mut self) -> &mut String {
        &mut self.buffer
    }

    fn get_operation_states(&self) -> Arc<RwLock<HashMap<OperationType, OperationState>>> {
        Arc::clone(&self.operation_states)
    }

    fn style_output(output: &str) -> String {
        console::style(output).dim().to_string()
    }
}

impl Write for ErrorStream {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        match &self.dest {
            OutputDest::Stdout => {
                self.buffer.push_str(s);
                self.flush();
            }

            OutputDest::Suppress => {
                // Do nothing.
            }

            OutputDest::BufferForTest(_) => {
                // Drop the error output, as the buffer only represents `stdout`.
            }
        }
        Ok(())
    }
}

impl Drop for ErrorStream {
    fn drop(&mut self) {
        WriteProgress::drop(self);
    }
}

pub struct ProgressHandle<'a> {
    effects: &'a Effects,
    operation_type: OperationType,
}

impl Drop for ProgressHandle<'_> {
    fn drop(&mut self) {
        self.effects
            .on_drop_progress_handle(self.operation_type.clone())
    }
}

impl ProgressHandle<'_> {
    pub fn notify_progress(&self, current: usize, total: usize) {
        self.effects
            .on_notify_progress(self.operation_type.clone(), current, total);
    }

    pub fn notify_progress_inc(&self, increment: usize) {
        self.effects
            .on_notify_progress_inc(self.operation_type.clone(), increment);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_effects_progress() -> eyre::Result<()> {
        let effects = Effects::new(Glyphs::text());
        let (effects2, progress2) = effects.start_operation(OperationType::GetMergeBase);

        {
            let operation_states = effects.operation_states.read().unwrap();
            let get_merge_base_operation =
                operation_states.get(&OperationType::GetMergeBase).unwrap();
            assert_eq!(get_merge_base_operation.start_times.len(), 1);
        }

        std::thread::sleep(Duration::from_millis(1));
        let (_effects3, progress3) = effects.start_operation(OperationType::GetMergeBase);
        let earlier_start_time = {
            let operation_states = effects.operation_states.read().unwrap();
            let get_merge_base_operation =
                operation_states.get(&OperationType::GetMergeBase).unwrap();
            assert_eq!(get_merge_base_operation.start_times.len(), 2);
            get_merge_base_operation.start_times[0]
        };

        drop(progress3);
        {
            let operation_states = effects.operation_states.read().unwrap();
            let get_merge_base_operation =
                operation_states.get(&OperationType::GetMergeBase).unwrap();
            // Ensure that we try to keep the earliest times in the list, to
            // accurately gauge the wall-clock time.
            assert_eq!(
                get_merge_base_operation.start_times,
                vec![earlier_start_time]
            );
        }

        // Nest an operation.
        let (_effects4, progress4) = effects2.start_operation(OperationType::CalculateDiff);
        std::thread::sleep(Duration::from_millis(1));
        drop(progress4);
        {
            let operation_states = effects.operation_states.read().unwrap();
            // The operation should still be present until the root-level
            // operation has finished, even if it's not currently in progress.
            let calculate_diff_operation =
                operation_states.get(&OperationType::CalculateDiff).unwrap();
            assert!(calculate_diff_operation.start_times.is_empty());
            assert!(calculate_diff_operation.elapsed_duration >= Duration::from_millis(1));
        }

        drop(progress2);
        {
            let operation_states = effects.operation_states.read().unwrap();
            assert!(operation_states.is_empty());
        }

        Ok(())
    }
}
