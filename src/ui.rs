//! UI implementation.

use crate::types::{RecordError, RecordState};

/// UI component to record the user's changes.
pub struct Recorder<'a> {
    _state: RecordState<'a>,
}

impl<'a> Recorder<'a> {
    /// Constructor.
    pub fn new(state: RecordState<'a>) -> Self {
        Self { _state: state }
    }

    /// Run the terminal user interface and have the user interactively select
    /// changes.
    pub fn run(self) -> Result<RecordState<'a>, RecordError> {
        todo!()
    }
}
