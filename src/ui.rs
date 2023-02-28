//! UI implementation.

use std::io;

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, is_raw_mode_enabled, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use tracing::warn;
use tui::layout::{Constraint, Direction, Layout};
use tui::widgets::Paragraph;
use tui::{backend::CrosstermBackend, Terminal};

use crate::types::{RecordError, RecordState};

type CrosstermTerminal = Terminal<CrosstermBackend<io::Stdout>>;

/// UI component to record the user's changes.
pub struct Recorder<'a> {
    state: RecordState<'a>,
}

impl<'a> Recorder<'a> {
    /// Constructor.
    pub fn new(state: RecordState<'a>) -> Self {
        Self { state }
    }

    /// Run the terminal user interface and have the user interactively select
    /// changes.
    pub fn run(self) -> Result<RecordState<'a>, RecordError> {
        let mut stdout = io::stdout();
        if !is_raw_mode_enabled().map_err(RecordError::SetUpTerminal)? {
            enable_raw_mode().map_err(RecordError::SetUpTerminal)?;
            crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
                .map_err(RecordError::SetUpTerminal)?;
        }
        let backend = CrosstermBackend::new(stdout);
        let mut term = Terminal::new(backend).map_err(RecordError::SetUpTerminal)?;

        let state = self.run_inner(&mut term)?;

        if let Err(err) = Self::clean_up(&mut term) {
            warn!(?err, "Failed to clean up terminal");
        }
        Ok(state)
    }

    fn run_inner(self, term: &mut CrosstermTerminal) -> Result<RecordState<'a>, RecordError> {
        loop {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1)]);

            term.draw(|frame| {
                let chunks = layout.split(frame.size());

                frame.render_widget(Paragraph::new("Hello, world!"), chunks[0]);
            })
            .map_err(RecordError::RenderFrame)?;

            match crossterm::event::read().map_err(RecordError::ReadInput)? {
                Event::Key(KeyEvent {
                    code: KeyCode::Char('q'),
                    modifiers: KeyModifiers::NONE,
                    kind: KeyEventKind::Press,
                    state: _,
                }) => break,

                _event => {}
            }
        }

        Ok(self.state)
    }

    fn clean_up(term: &mut CrosstermTerminal) -> io::Result<()> {
        term.show_cursor()?;
        if is_raw_mode_enabled()? {
            disable_raw_mode()?;
            crossterm::execute!(
                term.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture
            )?;
        }
        Ok(())
    }
}
