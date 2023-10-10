//! Helper functions for rendering UI components.

use std::time::Duration;

use crate::{Event, RecordError, RecordInput, TerminalKind};

/// Generate a one-line description of a binary file change.
pub fn make_binary_description(hash: &str, num_bytes: u64) -> String {
    format!("{} ({} bytes)", hash, num_bytes)
}

/// Reads input events from the terminal using `crossterm`.
pub struct CrosstermInput;

impl RecordInput for CrosstermInput {
    fn terminal_kind(&self) -> TerminalKind {
        TerminalKind::Crossterm
    }

    fn next_events(&mut self) -> Result<Vec<Event>, RecordError> {
        // Ensure we block for at least one event.
        let first_event = crossterm::event::read().map_err(RecordError::ReadInput)?;
        let mut events = vec![first_event.into()];
        // Some events, like scrolling, are generated more quickly than
        // we can render the UI. In those cases, batch up all available
        // events and process them before the next render.
        while crossterm::event::poll(Duration::ZERO).map_err(RecordError::ReadInput)? {
            let event = crossterm::event::read().map_err(RecordError::ReadInput)?;
            events.push(event.into());
        }
        Ok(events)
    }
}

/// Reads events from the provided sequence of events.
pub struct TestingInput {
    /// The width of the virtual terminal in columns.
    pub width: usize,

    /// The height of the virtual terminal in columns.
    pub height: usize,

    /// The sequence of events to emit.
    pub events: Box<dyn Iterator<Item = Event>>,
}

impl TestingInput {
    /// Helper function to construct a `TestingInput`.
    pub fn new(
        width: usize,
        height: usize,
        events: impl IntoIterator<Item = Event> + 'static,
    ) -> Self {
        Self {
            width,
            height,
            events: Box::new(events.into_iter()),
        }
    }
}

impl RecordInput for TestingInput {
    fn terminal_kind(&self) -> TerminalKind {
        let Self {
            width,
            height,
            events: _,
        } = self;
        TerminalKind::Testing {
            width: *width,
            height: *height,
        }
    }

    fn next_events(&mut self) -> Result<Vec<Event>, RecordError> {
        Ok(vec![self.events.next().unwrap_or(Event::None)])
    }
}
