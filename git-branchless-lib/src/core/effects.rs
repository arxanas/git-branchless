//! Wrappers around various side effects.

use bstr::ByteSlice;
use std::fmt::{Debug, Display, Write};
use std::io::{stderr, stdout, Stderr, Stdout, Write as WriteIo};
use std::mem::take;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use std::{io, thread};

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
    ConstrainCommits,
    DetectDuplicateCommits,
    EvaluateRevset(Arc<String>),
    FilterByTouchedPaths,
    FilterCommits,
    FindPathToMergeBase,
    GetMergeBase,
    GetTouchedPaths,
    GetUpstreamPatchIds,
    InitializeRebase,
    MakeGraph,
    ProcessEvents,
    PushCommits,
    QueryWorkingCopy,
    ReadingFromCache,
    RebaseCommits,
    RepairBranches,
    RepairCommits,
    RunGitCommand(Arc<String>),
    RunTestOnCommit(Arc<String>),
    RunTests(Arc<String>),
    SortCommits,
    SyncCommits,
    UpdateCommitGraph,
    UpdateCommits,
    WalkCommits,
}

impl Display for OperationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperationType::BuildRebasePlan => write!(f, "Building rebase plan"),
            OperationType::CalculateDiff => write!(f, "Computing diffs"),
            OperationType::CalculatePatchId => write!(f, "Hashing commit contents"),
            OperationType::CheckForCycles => write!(f, "Checking for cycles"),
            OperationType::ConstrainCommits => write!(f, "Creating commit constraints"),
            OperationType::DetectDuplicateCommits => write!(f, "Checking for duplicate commits"),
            OperationType::EvaluateRevset(revset) => {
                write!(f, "Evaluating revset: {revset}")
            }
            OperationType::FilterByTouchedPaths => {
                write!(f, "Filtering upstream commits by touched paths")
            }
            OperationType::FilterCommits => write!(f, "Filtering commits"),
            OperationType::FindPathToMergeBase => write!(f, "Finding path to merge-base"),
            OperationType::GetMergeBase => write!(f, "Calculating merge-bases"),
            OperationType::GetTouchedPaths => write!(f, "Getting touched paths"),
            OperationType::GetUpstreamPatchIds => write!(f, "Enumerating patch IDs"),
            OperationType::InitializeRebase => write!(f, "Initializing rebase"),
            OperationType::MakeGraph => write!(f, "Examining local history"),
            OperationType::PushCommits => write!(f, "Pushing branches"),
            OperationType::ProcessEvents => write!(f, "Processing events"),
            OperationType::QueryWorkingCopy => write!(f, "Querying the working copy"),
            OperationType::ReadingFromCache => write!(f, "Reading from cache"),
            OperationType::RebaseCommits => write!(f, "Rebasing commits"),
            OperationType::RepairBranches => write!(f, "Checking for broken branches"),
            OperationType::RepairCommits => write!(f, "Checking for broken commits"),
            OperationType::RunGitCommand(command) => {
                write!(f, "Running Git command: {}", &command)
            }
            OperationType::RunTests(command) => write!(f, "Running command: {command}"),
            OperationType::RunTestOnCommit(commit) => write!(f, "Waiting to run on {commit}"),
            OperationType::SortCommits => write!(f, "Sorting commits"),
            OperationType::SyncCommits => write!(f, "Syncing commit stacks"),
            OperationType::UpdateCommits => write!(f, "Updating commits"),
            OperationType::UpdateCommitGraph => write!(f, "Updating commit graph"),
            OperationType::WalkCommits => write!(f, "Walking commits"),
        }
    }
}

#[derive(Clone, Debug)]
enum OutputDest {
    Stdout,
    Suppress,
    BufferForTest {
        stdout: Arc<Mutex<Vec<u8>>>,
        stderr: Arc<Mutex<Vec<u8>>>,
    },
}

/// An index into the recursive hierarchy of progress bars. For example, the key
/// `[OperationType::GetMergeBase, OperationType::WalkCommits]` refers to the
/// "walk commits" operation which is nested under the "get merge-base"
/// operation.
type OperationKey = [OperationType];

#[derive(Debug, Default)]
struct RootOperation {
    multi_progress: MultiProgress,
    children: Vec<OperationState>,
}

impl RootOperation {
    pub fn hide_multi_progress(&mut self) {
        self.multi_progress
            .set_draw_target(ProgressDrawTarget::hidden());
    }

    pub fn show_multi_progress(&mut self) {
        self.multi_progress
            .set_draw_target(ProgressDrawTarget::stderr());
    }

    /// If all operations are no longer in progress, clear the multi-progress bar.
    pub fn clear_operations_if_finished(&mut self) {
        if self
            .children
            .iter()
            .all(|operation_state| operation_state.start_times.is_empty())
        {
            if self.multi_progress.clear().is_err() {
                // Ignore error. Assume that the draw target is no longer available
                // to write to.
            }
            self.children.clear();
        }
    }

    pub fn get_or_create_child(&mut self, key: &[OperationType]) -> &mut OperationState {
        match key {
            [] => panic!("Empty operation key"),
            [first, rest @ ..] => {
                let index = match self
                    .children
                    .iter()
                    .find_position(|child| &child.operation_type == first)
                {
                    Some((child_index, _)) => child_index,
                    None => {
                        self.children.push(OperationState {
                            operation_type: first.clone(),
                            progress_bar: ProgressBar::new_spinner(),
                            has_meter: Default::default(),
                            icon: Default::default(),
                            progress_message: first.to_string(),
                            start_times: Default::default(),
                            elapsed_duration: Default::default(),
                            children: Default::default(),
                        });
                        self.children.len() - 1
                    }
                };
                self.children
                    .get_mut(index)
                    .unwrap()
                    .get_or_create_child(rest)
            }
        }
    }

    pub fn get_child(&mut self, key: &[OperationType]) -> Option<&mut OperationState> {
        match key {
            [] => panic!("Empty operation key"),
            [first, rest @ ..] => {
                let index = self
                    .children
                    .iter()
                    .find_position(|child| &child.operation_type == first);
                match index {
                    Some((index, _)) => self.children.get_mut(index).unwrap().get_child(rest),
                    None => None,
                }
            }
        }
    }

    /// Re-render all operation progress bars. This does not change their
    /// ordering like [`refresh_multi_progress`] does.
    pub fn tick(&mut self) {
        let operations = {
            let mut acc = Vec::new();
            Self::traverse_operations(&mut acc, 0, &self.children);
            acc
        };
        for (nesting_level, operation) in operations {
            operation.tick(nesting_level);
        }
    }

    /// Update the ordering of progress bars in the multi-progress. This should be called after
    pub fn refresh_multi_progress(&mut self) {
        let operations = {
            let mut acc = Vec::new();
            Self::traverse_operations(&mut acc, 0, &self.children);
            acc
        };
        if self.multi_progress.clear().is_err() {
            // Ignore the error and assume that the multi-progress is now dead,
            // so it doesn't need to be updated.
        }
        for (nesting_level, operation) in operations {
            // Avoid deadlock inside the progress bar library when we call
            // `add`, which sets the draw target again.
            operation
                .progress_bar
                .set_draw_target(ProgressDrawTarget::hidden());

            self.multi_progress.add(operation.progress_bar.clone());

            // Re-render only after it's been added to the multi-progress, so
            // that the draw target has been set.
            operation.tick(nesting_level);
        }
    }

    fn traverse_operations<'a>(
        acc: &mut Vec<(usize, &'a OperationState)>,
        current_level: usize,
        operations: &'a [OperationState],
    ) {
        for operation in operations {
            acc.push((current_level, operation));
            Self::traverse_operations(acc, current_level + 1, &operation.children);
        }
    }
}

/// The string values associated with [`OperationIcon`]s.
pub mod icons {
    /// Used to indicate success.
    pub const CHECKMARK: &str = "✓";

    /// Used to indicate a warning.
    pub const EXCLAMATION: &str = "!";

    /// Used to indicate failure.
    ///
    /// Can't use "✗️" in interactive progress meters because some terminals think its width is >1,
    /// which seems to cause rendering issues because we use 1 as its width.
    pub const CROSS: &str = "X";
}

/// An icon denoting the status of an operation.
#[derive(Clone, Copy, Debug)]
pub enum OperationIcon {
    /// A suitable waiting icon should be rendered.
    InProgress,

    /// The operation was a success.
    Success,

    /// The operation produced a warning.
    Warning,

    /// The operation was a failure.
    Failure,
}

impl Default for OperationIcon {
    fn default() -> Self {
        Self::InProgress
    }
}

#[derive(Debug)]
struct OperationState {
    operation_type: OperationType,
    progress_bar: ProgressBar,
    has_meter: bool,
    start_times: Vec<Instant>,
    icon: OperationIcon,
    progress_message: String,
    elapsed_duration: Duration,
    children: Vec<OperationState>,
}

impl OperationState {
    pub fn set_progress(&mut self, current: usize, total: usize) {
        self.has_meter = true;

        if current
            .try_into()
            .map(|current: u64| current < self.progress_bar.position())
            .unwrap_or(false)
        {
            // Workaround for issue fixed by
            // <https://github.com/console-rs/indicatif/pull/403>.
            self.progress_bar.reset_eta();
        }

        self.progress_bar.set_position(current.try_into().unwrap());
        self.progress_bar.set_length(total.try_into().unwrap());
    }

    pub fn inc_progress(&mut self, increment: usize) {
        self.progress_bar.inc(increment.try_into().unwrap());
    }

    pub fn get_or_create_child(&mut self, child_type: &[OperationType]) -> &mut Self {
        match child_type {
            [] => self,
            [first, rest @ ..] => {
                let index = match self
                    .children
                    .iter()
                    .find_position(|child| &child.operation_type == first)
                {
                    Some((child_index, _)) => child_index,
                    None => {
                        self.children.push(OperationState {
                            operation_type: first.clone(),
                            progress_bar: ProgressBar::new_spinner(),
                            has_meter: Default::default(),
                            icon: Default::default(),
                            progress_message: first.to_string(),
                            start_times: Default::default(),
                            elapsed_duration: Default::default(),
                            children: Default::default(),
                        });
                        self.children.len() - 1
                    }
                };
                self.children
                    .get_mut(index)
                    .unwrap()
                    .get_or_create_child(rest)
            }
        }
    }

    pub fn get_child(&mut self, child_type: &[OperationType]) -> Option<&mut Self> {
        match child_type {
            [] => Some(self),
            [first, rest @ ..] => {
                let index = self
                    .children
                    .iter()
                    .find_position(|child| &child.operation_type == first);
                match index {
                    Some((index, _)) => self.children.get_mut(index).unwrap().get_child(rest),
                    None => None,
                }
            }
        }
    }

    pub fn tick(&self, nesting_level: usize) {
        lazy_static! {
            static ref CHECKMARK: String = console::style(icons::CHECKMARK).green().to_string();
            static ref EXCLAMATION: String = console::style(icons::EXCLAMATION).yellow().to_string();
            static ref CROSS: String = console::style(icons::CROSS).red().to_string();
            static ref IN_PROGRESS_SPINNER_STYLE: Arc<Mutex<ProgressStyle>> =
                Arc::new(Mutex::new(ProgressStyle::default_spinner().template("{prefix}{spinner} {wide_msg}").unwrap()));
            static ref IN_PROGRESS_BAR_STYLE: Arc<Mutex<ProgressStyle>> =
                // indicatif places the cursor at the end of the line, which may
                // be visible in the terminal, so we add a space at the end of
                // the line so that the length number isn't overlapped by the
                // cursor.
                Arc::new(Mutex::new(ProgressStyle::default_bar().template("{prefix}{spinner} {wide_msg} {bar} {pos}/{len} ").unwrap()));
            static ref WAITING_PROGRESS_STYLE: Arc<Mutex<ProgressStyle>> = Arc::new(Mutex::new(IN_PROGRESS_SPINNER_STYLE
                .clone().lock().unwrap().clone()
                // Requires at least two tick values, so just pass the same one twice.
                .tick_strings(&[" ", " "])
            ));
            static ref SUCCESS_PROGRESS_STYLE: Arc<Mutex<ProgressStyle>> = Arc::new(Mutex::new(IN_PROGRESS_SPINNER_STYLE
                .clone()
                .lock()
                .unwrap()
                .clone()
                // Requires at least two tick values, so just pass the same one twice.
                .tick_strings(&[&CHECKMARK, &CHECKMARK])));
            static ref WARNING_PROGRESS_STYLE: Arc<Mutex<ProgressStyle>> = Arc::new(Mutex::new(IN_PROGRESS_SPINNER_STYLE
                .clone()
                .lock()
                .unwrap()
                .clone()
                // Requires at least two tick values, so just pass the same one twice.
                .tick_strings(&[&CROSS, &CROSS])));
            static ref FAILURE_PROGRESS_STYLE: Arc<Mutex<ProgressStyle>> = Arc::new(Mutex::new(IN_PROGRESS_SPINNER_STYLE
                .clone()
                .lock()
                .unwrap()
                .clone()
                // Requires at least two tick values, so just pass the same one twice.
                .tick_strings(&[&CROSS, &CROSS])));
        }

        let elapsed_duration = match self.start_times.iter().min() {
            None => self.elapsed_duration,
            Some(start_time) => {
                let additional_duration = Instant::now().saturating_duration_since(*start_time);
                self.elapsed_duration + additional_duration
            }
        };

        self.progress_bar.set_style(
            match (self.start_times.as_slice(), self.has_meter, self.icon) {
                (_, _, OperationIcon::Success) => SUCCESS_PROGRESS_STYLE.lock().unwrap().clone(),
                (_, _, OperationIcon::Warning) => WARNING_PROGRESS_STYLE.lock().unwrap().clone(),
                (_, _, OperationIcon::Failure) => FAILURE_PROGRESS_STYLE.lock().unwrap().clone(),
                ([], _, OperationIcon::InProgress) => {
                    WAITING_PROGRESS_STYLE.lock().unwrap().clone()
                }
                ([..], false, OperationIcon::InProgress) => {
                    IN_PROGRESS_SPINNER_STYLE.lock().unwrap().clone()
                }
                ([..], true, OperationIcon::InProgress) => {
                    IN_PROGRESS_BAR_STYLE.lock().unwrap().clone()
                }
            },
        );
        self.progress_bar.set_prefix("  ".repeat(nesting_level));
        self.progress_bar.set_message(format!(
            "{} ({:.1}s)",
            self.progress_message,
            elapsed_duration.as_secs_f64(),
        ));
        self.progress_bar.tick();
    }
}

/// Wrapper around side-effectful operations, such as output and progress
/// indicators.
#[derive(Clone)]
pub struct Effects {
    glyphs: Glyphs,
    dest: OutputDest,
    updater_thread_handle: Arc<RwLock<UpdaterThreadHandle>>,
    operation_key: Vec<OperationType>,
    root_operation: Arc<Mutex<RootOperation>>,
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

#[derive(Clone, Debug, Default)]
struct UpdaterThreadHandle {
    is_visible: bool,
}

fn spawn_progress_updater_thread(
    root_operation: &Arc<Mutex<RootOperation>>,
) -> Arc<RwLock<UpdaterThreadHandle>> {
    {
        let mut root_operation = root_operation.lock().unwrap();
        root_operation.hide_multi_progress();
    }
    let root_operation = Arc::downgrade(root_operation);
    let handle = Arc::new(RwLock::new(UpdaterThreadHandle { is_visible: false }));

    thread::spawn({
        let handle = Arc::clone(&handle);
        move || {
            // Don't start displaying progress immediately, since if the operation
            // finishes quickly, then it will flicker annoyingly.
            thread::sleep(Duration::from_millis(250));
            {
                let mut handle = handle.write().unwrap();
                match root_operation.upgrade() {
                    Some(root_operation) => {
                        let mut root_operation = root_operation.lock().unwrap();
                        root_operation.show_multi_progress();
                    }
                    None => return,
                }
                handle.is_visible = true;
            }

            loop {
                // Drop the `Arc` after this block, before the sleep, to make sure
                // that progress bars aren't kept alive longer than they should be.
                match root_operation.upgrade() {
                    None => return,
                    Some(root_operation) => {
                        let mut root_operation = root_operation.lock().unwrap();
                        root_operation.tick();
                    }
                }

                thread::sleep(Duration::from_millis(100));
            }
        }
    });

    handle
}

impl Effects {
    /// Constructor. Writes to stdout.
    pub fn new(glyphs: Glyphs) -> Self {
        let root_operation = Default::default();
        let updater_thread_handle = spawn_progress_updater_thread(&root_operation);
        Effects {
            glyphs,
            dest: OutputDest::Stdout,
            updater_thread_handle,
            operation_key: Default::default(),
            root_operation,
        }
    }

    /// Constructor. Suppresses all output.
    pub fn new_suppress_for_test(glyphs: Glyphs) -> Self {
        Effects {
            glyphs,
            dest: OutputDest::Suppress,
            updater_thread_handle: Default::default(),
            operation_key: Default::default(),
            root_operation: Default::default(),
        }
    }

    /// Constructor. Writes to the provided buffer.
    pub fn new_from_buffer_for_test(
        glyphs: Glyphs,
        stdout: &Arc<Mutex<Vec<u8>>>,
        stderr: &Arc<Mutex<Vec<u8>>>,
    ) -> Self {
        Effects {
            glyphs,
            dest: OutputDest::BufferForTest {
                stdout: Arc::clone(stdout),
                stderr: Arc::clone(stderr),
            },
            updater_thread_handle: Default::default(),
            operation_key: Default::default(),
            root_operation: Default::default(),
        }
    }

    /// Send output to an appropriate place when using a terminal user interface
    /// (TUI), such as for `git undo`.
    pub fn enable_tui_mode(&self) -> Self {
        let mut root_operation = self.root_operation.lock().unwrap();
        root_operation.hide_multi_progress();
        Self {
            dest: OutputDest::Suppress,
            ..self.clone()
        }
    }

    /// Suppress output sent to the returned `Effects`.
    pub fn suppress(&self) -> Self {
        Self {
            dest: OutputDest::Suppress,
            ..self.clone()
        }
    }

    /// Apply transformations to the returned `Effects` to support emitting
    /// graphical output in the opposite of its usual order.
    pub fn reverse_order(&self, reverse: bool) -> Self {
        Self {
            glyphs: self.glyphs.clone().reverse_order(reverse),
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
    pub fn start_operation(&self, operation_type: OperationType) -> (Effects, ProgressHandle<'_>) {
        let operation_key = {
            let mut result = self.operation_key.clone();
            result.push(operation_type);
            result
        };
        let progress = ProgressHandle {
            effects: self,
            operation_key: operation_key.clone(),
        };
        match self.dest {
            OutputDest::Stdout => {}
            OutputDest::Suppress | OutputDest::BufferForTest { .. } => {
                return (self.clone(), progress)
            }
        }

        let now = Instant::now();
        let mut root_operation = self.root_operation.lock().unwrap();
        let operation_state = root_operation.get_or_create_child(&operation_key);
        operation_state.start_times.push(now);
        root_operation.refresh_multi_progress();

        let effects = Self {
            operation_key,
            ..self.clone()
        };
        (effects, progress)
    }

    fn on_notify_progress(&self, operation_key: &OperationKey, current: usize, total: usize) {
        match self.dest {
            OutputDest::Stdout => {}
            OutputDest::Suppress | OutputDest::BufferForTest { .. } => return,
        }

        let mut root_operation = self.root_operation.lock().unwrap();
        let operation_state = root_operation.get_or_create_child(operation_key);
        operation_state.set_progress(current, total);
    }

    fn on_notify_progress_inc(&self, operation_key: &OperationKey, increment: usize) {
        match self.dest {
            OutputDest::Stdout => {}
            OutputDest::Suppress | OutputDest::BufferForTest { .. } => return,
        }

        let mut root_operation = self.root_operation.lock().unwrap();
        let operation = root_operation.get_child(operation_key);
        let operation_state = match operation {
            Some(operation_state) => operation_state,
            None => return,
        };
        operation_state.inc_progress(increment);
    }

    fn on_set_message(&self, operation_key: &OperationKey, icon: OperationIcon, message: String) {
        match self.dest {
            OutputDest::Stdout => {}
            OutputDest::Suppress | OutputDest::BufferForTest { .. } => return,
        }

        let mut root_operation = self.root_operation.lock().unwrap();
        let operation = root_operation.get_child(operation_key);
        let operation_state = match operation {
            Some(operation_state) => operation_state,
            None => return,
        };
        operation_state.icon = icon;
        operation_state.progress_message = message;
    }

    fn on_drop_progress_handle(&self, operation_key: &OperationKey) {
        match self.dest {
            OutputDest::Stdout => {}
            OutputDest::Suppress | OutputDest::BufferForTest { .. } => return,
        }

        let now = Instant::now();
        let mut root_operation = self.root_operation.lock().unwrap();

        let operation = root_operation.get_child(operation_key);
        let operation_state = match operation {
            Some(operation_state) => operation_state,
            None => {
                drop(root_operation); // Avoid potential deadlock.
                warn!(?operation_key, "Progress operation not started");
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
                drop(root_operation); // Avoid potential deadlock.
                warn!(
                    ?operation_key,
                    "Progress operation ended without matching start call"
                );
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

        root_operation.clear_operations_if_finished();
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
            updater_thread_handle: Arc::clone(&self.updater_thread_handle),
            root_operation: Arc::clone(&self.root_operation),
        }
    }

    /// Create a stream that error output can be written to, rather than regular
    /// output.
    pub fn get_error_stream(&self) -> ErrorStream {
        ErrorStream {
            dest: self.dest.clone(),
            buffer: Default::default(),
            updater_thread_handle: Arc::clone(&self.updater_thread_handle),
            root_operation: Arc::clone(&self.root_operation),
        }
    }
}

trait WriteProgress {
    type Stream: WriteIo;
    fn get_stream() -> Self::Stream;
    fn get_buffer(&mut self) -> &mut String;
    fn get_root_operation(&self) -> Arc<Mutex<RootOperation>>;
    fn get_updater_thread_handle(&self) -> Arc<RwLock<UpdaterThreadHandle>>;
    fn style_output(output: &str) -> String;

    fn flush(&mut self) {
        let root_operation = self.get_root_operation();
        let root_operation = root_operation.lock().unwrap();

        // Get an arbitrary progress meter. It turns out that when a
        // `ProgressBar` is included in a `MultiProgress`, it doesn't matter
        // which of them we call `println` on. The output will be printed above
        // the `MultiProgress` regardless.
        let operation = root_operation.children.first();
        match operation {
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

            Some(_operation_state)
                if !self.get_updater_thread_handle().read().unwrap().is_visible =>
            {
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
                    let lines = self.get_buffer().split_inclusive('\n');
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
        };
    }

    fn drop(&mut self) {
        let buffer = self.get_buffer();
        if !buffer.is_empty() {
            // NB: this only flushes completely-written lines, which is not
            // correct. We should flush the entire buffer contents, and possibly
            // force a newline at the end in the case of a progress meter being
            // visible.
            self.flush();
        }
    }
}

/// A handle to stdout, but doesn't overwrite interactive progress notifications.
pub struct OutputStream {
    dest: OutputDest,
    buffer: String,
    updater_thread_handle: Arc<RwLock<UpdaterThreadHandle>>,
    root_operation: Arc<Mutex<RootOperation>>,
}

impl WriteProgress for OutputStream {
    type Stream = Stdout;

    fn get_stream() -> Self::Stream {
        stdout()
    }

    fn get_buffer(&mut self) -> &mut String {
        &mut self.buffer
    }

    fn get_root_operation(&self) -> Arc<Mutex<RootOperation>> {
        Arc::clone(&self.root_operation)
    }

    fn get_updater_thread_handle(&self) -> Arc<RwLock<UpdaterThreadHandle>> {
        Arc::clone(&self.updater_thread_handle)
    }

    fn style_output(output: &str) -> String {
        let output = console::strip_ansi_codes(output);
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

            OutputDest::BufferForTest { stdout, stderr: _ } => {
                let mut buffer = stdout.lock().unwrap();
                write!(buffer, "{s}").unwrap();
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

/// A handle to stderr, but doesn't overwrite interactive progress notifications.
pub struct ErrorStream {
    dest: OutputDest,
    buffer: String,
    updater_thread_handle: Arc<RwLock<UpdaterThreadHandle>>,
    root_operation: Arc<Mutex<RootOperation>>,
}

impl WriteProgress for ErrorStream {
    type Stream = Stderr;

    fn get_stream() -> Self::Stream {
        stderr()
    }

    fn get_buffer(&mut self) -> &mut String {
        &mut self.buffer
    }

    fn get_root_operation(&self) -> Arc<Mutex<RootOperation>> {
        Arc::clone(&self.root_operation)
    }

    fn get_updater_thread_handle(&self) -> Arc<RwLock<UpdaterThreadHandle>> {
        Arc::clone(&self.updater_thread_handle)
    }

    fn style_output(output: &str) -> String {
        let output = console::strip_ansi_codes(output);
        console::style(output).dim().to_string()
    }
}

impl Write for ErrorStream {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        match &self.dest {
            OutputDest::Stdout => {
                self.buffer.push_str(s);
                WriteProgress::flush(self);
            }

            OutputDest::Suppress => {
                // Do nothing.
            }

            OutputDest::BufferForTest { stdout: _, stderr } => {
                let mut buffer = stderr.lock().unwrap();
                write!(buffer, "{s}").unwrap();
            }
        }
        Ok(())
    }
}

/// You probably don't want this. This implementation is only for `tracing`'s `fmt_layer`, because
/// it needs a writer of type `io::Write`, but `Effects` normally uses its implementation of
/// `fmt::Write`.
impl io::Write for ErrorStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &self.dest {
            OutputDest::Stdout => {
                self.buffer.push_str(buf.to_str_lossy().as_ref());
                Ok(buf.len())
            }
            OutputDest::Suppress => {
                // Do nothing.
                Ok(buf.len())
            }
            OutputDest::BufferForTest { stdout: _, stderr } => {
                let mut buffer = stderr.lock().unwrap();
                buffer.write(buf)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        WriteProgress::flush(self);
        Ok(())
    }
}

impl Drop for ErrorStream {
    fn drop(&mut self) {
        WriteProgress::drop(self);
    }
}

/// A handle to an operation in progress. This object should be kept live while
/// the operation is underway, and a timing entry for it will be displayed in
/// the interactive progress display.
#[derive(Debug)]
pub struct ProgressHandle<'a> {
    effects: &'a Effects,
    operation_key: Vec<OperationType>,
}

impl Drop for ProgressHandle<'_> {
    fn drop(&mut self) {
        self.effects.on_drop_progress_handle(&self.operation_key)
    }
}

impl ProgressHandle<'_> {
    /// Notify the progress meter that the current operation has `total`
    /// discrete units of work, and it's currently `current` units of the way
    /// through the operation.
    pub fn notify_progress(&self, current: usize, total: usize) {
        self.effects
            .on_notify_progress(&self.operation_key, current, total);
    }

    /// Notify the progress meter that additional progress has taken place.
    /// Should be used only after a call to `notify_progress` to indicate how
    /// much total work there is.
    pub fn notify_progress_inc(&self, increment: usize) {
        self.effects
            .on_notify_progress_inc(&self.operation_key, increment);
    }

    /// Update the message for this progress meter.
    pub fn notify_status(&self, icon: OperationIcon, message: impl Into<String>) {
        let message = message.into();
        self.effects
            .on_set_message(&self.operation_key, icon, message);
    }
}

/// A wrapper around an iterator that reports progress as it iterates.
pub struct ProgressIter<'a, TItem, TInner: Iterator<Item = TItem>> {
    index: usize,
    inner: TInner,
    progress: ProgressHandle<'a>,
}

impl<TItem, TInner: Iterator<Item = TItem>> Iterator for ProgressIter<'_, TItem, TInner> {
    type Item = TItem;

    fn next(&mut self) -> Option<Self::Item> {
        let (lower, upper) = self.inner.size_hint();
        let size_guess = upper.unwrap_or(lower);
        self.progress.notify_progress(self.index, size_guess);
        self.index += 1;
        self.inner.next()
    }
}

/// Extension trait for iterators that adds a `with_progress` method.
pub trait WithProgress<'a, TItem>: Iterator<Item = TItem> {
    /// The type of the iterator returned by `with_progress`.
    type Iter: Iterator<Item = TItem> + 'a;

    /// Wrap the iterator into an iterator that reports progress as it consumes items.
    fn with_progress(self, progress: ProgressHandle<'a>) -> Self::Iter;
}

impl<'a, TItem: 'a, TIter: Iterator<Item = TItem> + 'a> WithProgress<'a, TItem> for TIter {
    type Iter = ProgressIter<'a, TItem, TIter>;

    fn with_progress(self, progress: ProgressHandle<'a>) -> Self::Iter {
        ProgressIter {
            index: 0,
            inner: self,
            progress,
        }
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
            let mut root_operation = effects.root_operation.lock().unwrap();
            let get_merge_base_operation = root_operation
                .get_child(&[OperationType::GetMergeBase])
                .unwrap();
            assert_eq!(get_merge_base_operation.start_times.len(), 1);
        }

        std::thread::sleep(Duration::from_millis(1));
        let (_effects3, progress3) = effects.start_operation(OperationType::GetMergeBase);
        let earlier_start_time = {
            let mut root_operation = effects.root_operation.lock().unwrap();
            let get_merge_base_operation = root_operation
                .get_child(&[OperationType::GetMergeBase])
                .ok_or_else(|| eyre::eyre!("Could not find merge-base operation"))?;
            assert_eq!(get_merge_base_operation.start_times.len(), 2);
            get_merge_base_operation.start_times[0]
        };

        drop(progress3);
        {
            let mut root_operation = effects.root_operation.lock().unwrap();
            let get_merge_base_operation = root_operation
                .get_child(&[OperationType::GetMergeBase])
                .unwrap();
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
            let mut root_operation = effects.root_operation.lock().unwrap();
            // The operation should still be present until the root-level
            // operation has finished, even if it's not currently in progress.
            let calculate_diff_operation = root_operation
                .get_child(&[OperationType::GetMergeBase, OperationType::CalculateDiff])
                .unwrap();
            assert!(calculate_diff_operation.start_times.is_empty());
            assert!(calculate_diff_operation.elapsed_duration >= Duration::from_millis(1));
        }

        drop(progress2);
        {
            let root_operation = effects.root_operation.lock().unwrap();
            assert!(root_operation.children.is_empty());
        }

        Ok(())
    }

    /// Test for the issue fixed by <https://github.com/console-rs/indicatif/pull/403>.
    #[test]
    fn test_effects_progress_rewind_panic() -> eyre::Result<()> {
        let effects = Effects::new(Glyphs::text());
        let (effects, progress) = effects.start_operation(OperationType::GetMergeBase);
        let _ = effects;
        progress.notify_progress(3, 10);
        // Should not panic.
        progress.notify_progress(0, 10);
        Ok(())
    }
}
