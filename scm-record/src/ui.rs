//! UI implementation.

use std::any::Any;
use std::borrow::Cow;
use std::cell::RefCell;
use std::cmp::min;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;
use std::{fs, io, panic};

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, is_raw_mode_enabled, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use tui::backend::{Backend, TestBackend};
use tui::buffer::Buffer;
use tui::style::{Color, Modifier, Style};
use tui::text::Span;
use tui::widgets::{Block, Borders, Clear, Paragraph};
use tui::{backend::CrosstermBackend, Terminal};
use unicode_width::UnicodeWidthStr;

use crate::consts::{DUMP_UI_STATE_FILENAME, ENV_VAR_DEBUG_UI, ENV_VAR_DUMP_UI_STATE};
use crate::render::{centered_rect, Component, Rect, RectSize, Viewport};
use crate::types::{ChangeType, RecordError, RecordState};
use crate::util::UsizeExt;
use crate::{File, Section, SectionChangedLine};

const NUM_CONTEXT_LINES: usize = 3;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
struct FileKey {
    file_idx: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
struct SectionKey {
    file_idx: usize,
    section_idx: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
struct LineKey {
    file_idx: usize,
    section_idx: usize,
    line_idx: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
enum QuitDialogButtonId {
    Quit,
    GoBack,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
enum SelectionKey {
    None,
    File(FileKey),
    Section(SectionKey),
    Line(LineKey),
}

impl Default for SelectionKey {
    fn default() -> Self {
        Self::None
    }
}

/// A copy of the contents of the screen at a certain point in time.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TestingScreenshot {
    contents: Rc<RefCell<Option<String>>>,
}

impl TestingScreenshot {
    fn set(&self, new_contents: String) {
        let Self { contents } = self;
        *contents.borrow_mut() = Some(new_contents);
    }

    /// Produce an `Event` which will record the screenshot when it's handled.
    pub fn event(&self) -> Event {
        Event::TakeScreenshot(self.clone())
    }
}

impl Display for TestingScreenshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { contents } = self;
        match contents.borrow().as_ref() {
            Some(contents) => write!(f, "{contents}"),
            None => write!(f, "<this screenshot was never assigned>"),
        }
    }
}

#[allow(missing_docs)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    None,
    QuitAccept,
    QuitCancel,
    QuitInterrupt,
    TakeScreenshot(TestingScreenshot),
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    FocusPrev,
    FocusPrevPage,
    FocusNext,
    FocusNextPage,
    FocusInner,
    FocusOuter,
    ToggleItem,
    ToggleItemAndAdvance,
    ToggleAll,
    Click { row: usize, column: usize },
}

impl From<crossterm::event::Event> for Event {
    fn from(event: crossterm::event::Event) -> Self {
        use crossterm::event::Event;
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: _,
            }) => Self::QuitCancel,

            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                state: _,
            }) => Self::QuitInterrupt,

            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: _,
            }) => Self::QuitAccept,

            Event::Key(KeyEvent {
                code: KeyCode::Char('y'),
                modifiers: KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                state: _,
            })
            | Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: _,
                row: _,
                modifiers: _,
            }) => Self::ScrollUp,
            Event::Key(KeyEvent {
                code: KeyCode::Char('e'),
                modifiers: KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                state: _,
            })
            | Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: _,
                row: _,
                modifiers: _,
            }) => Self::ScrollDown,

            Event::Key(
                KeyEvent {
                    code: KeyCode::PageUp,
                    modifiers: KeyModifiers::NONE,
                    kind: KeyEventKind::Press,
                    state: _,
                }
                | KeyEvent {
                    code: KeyCode::Char('b'),
                    modifiers: KeyModifiers::CONTROL,
                    kind: KeyEventKind::Press,
                    state: _,
                },
            ) => Self::PageUp,
            Event::Key(
                KeyEvent {
                    code: KeyCode::PageDown,
                    modifiers: KeyModifiers::NONE,
                    kind: KeyEventKind::Press,
                    state: _,
                }
                | KeyEvent {
                    code: KeyCode::Char('f'),
                    modifiers: KeyModifiers::CONTROL,
                    kind: KeyEventKind::Press,
                    state: _,
                },
            ) => Self::PageDown,

            Event::Key(KeyEvent {
                code: KeyCode::Up,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: _,
            }) => Self::FocusPrev,

            Event::Key(KeyEvent {
                code: KeyCode::Down,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: _,
            }) => Self::FocusNext,

            Event::Key(KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: _,
            }) => Self::FocusOuter,

            Event::Key(KeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: _,
            }) => Self::FocusInner,

            Event::Key(KeyEvent {
                code: KeyCode::Char('u'),
                modifiers: KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                state: _,
            }) => Self::FocusPrevPage,
            Event::Key(KeyEvent {
                code: KeyCode::Char('d'),
                modifiers: KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                state: _,
            }) => Self::FocusNextPage,

            Event::Key(KeyEvent {
                code: KeyCode::Char(' '),
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: _,
            }) => Self::ToggleItem,

            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: _,
            }) => Self::ToggleItemAndAdvance,

            Event::Key(KeyEvent {
                code: KeyCode::Char('a'),
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: _,
            }) => Self::ToggleAll,

            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: _,
            }) => Self::Click {
                row: row.into(),
                column: column.into(),
            },

            _event => Self::None,
        }
    }
}

/// The source to read user events from.
pub enum EventSource {
    /// Read from the terminal with `crossterm`.
    Crossterm,

    /// Read from the provided sequence of events.
    Testing {
        /// The width of the virtual terminal in columns.
        width: usize,

        /// The height of the virtual terminal in columns.
        height: usize,

        /// The sequence of events to emit.
        events: Box<dyn Iterator<Item = Event>>,
    },
}

impl EventSource {
    /// Helper function to construct an `EventSource::Testing`.
    pub fn testing(
        width: usize,
        height: usize,
        events: impl IntoIterator<Item = Event> + 'static,
    ) -> Self {
        Self::Testing {
            width,
            height,
            events: Box::new(events.into_iter()),
        }
    }

    fn next_events(&mut self) -> Result<Vec<Event>, RecordError> {
        match self {
            EventSource::Crossterm => {
                // Some events, like scrolling, are generated more quickly than
                // we can render the UI. In those cases, batch up all available
                // events and process them before the next render.
                let mut events = Vec::new();
                while crossterm::event::poll(Duration::ZERO).map_err(RecordError::ReadInput)? {
                    let event = crossterm::event::read().map_err(RecordError::ReadInput)?;
                    events.push(event.into());
                }
                Ok(events)
            }
            EventSource::Testing {
                width: _,
                height: _,
                events,
            } => Ok(vec![events.next().unwrap_or(Event::None)]),
        }
    }
}

/// Copied from internal implementation of `tui`.
fn buffer_view(buffer: &Buffer) -> String {
    let mut view =
        String::with_capacity(buffer.content.len() + usize::from(buffer.area.height) * 3);
    for cells in buffer.content.chunks(buffer.area.width.into()) {
        let mut overwritten = vec![];
        let mut skip: usize = 0;
        view.push('"');
        for (x, c) in cells.iter().enumerate() {
            if skip == 0 {
                view.push_str(&c.symbol);
            } else {
                overwritten.push((x, &c.symbol))
            }
            skip = std::cmp::max(skip, c.symbol.width()).saturating_sub(1);
        }
        view.push('"');
        if !overwritten.is_empty() {
            write!(
                &mut view,
                " Hidden by multi-width symbols: {:?}",
                overwritten
            )
            .unwrap();
        }
        view.push('\n');
    }
    view
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum StateUpdate {
    None,
    SetQuitDialog(Option<QuitDialog>),
    QuitAccept,
    QuitCancel,
    TakeScreenshot(TestingScreenshot),
    ScrollTo(isize),
    SelectItem(SelectionKey),
    ToggleItem(SelectionKey),
    ToggleItemAndAdvance(SelectionKey, SelectionKey),
    ToggleAll,
}

/// UI component to record the user's changes.
pub struct Recorder<'a> {
    state: RecordState<'a>,
    event_source: EventSource,
    use_unicode: bool,
    selection_key: SelectionKey,
    quit_dialog: Option<QuitDialog>,
    scroll_offset_y: isize,
}

impl<'a> Recorder<'a> {
    /// Constructor.
    pub fn new(state: RecordState<'a>, event_source: EventSource) -> Self {
        Self {
            state,
            event_source,
            use_unicode: true,
            selection_key: SelectionKey::None,
            quit_dialog: None,
            scroll_offset_y: 0,
        }
    }

    /// Run the terminal user interface and have the user interactively select
    /// changes.
    pub fn run(self) -> Result<RecordState<'a>, RecordError> {
        #[cfg(feature = "debug")]
        if std::env::var_os(ENV_VAR_DUMP_UI_STATE).is_some() {
            let ui_state =
                serde_json::to_string_pretty(&self.state).map_err(RecordError::SerializeJson)?;
            fs::write(DUMP_UI_STATE_FILENAME, ui_state).map_err(RecordError::WriteFile)?;
        }

        match self.event_source {
            EventSource::Crossterm => self.run_crossterm(),
            EventSource::Testing {
                width,
                height,
                events: _,
            } => self.run_testing(width, height),
        }
    }

    /// Run the recorder UI using `crossterm` as the backend connected to stdout.
    fn run_crossterm(self) -> Result<RecordState<'a>, RecordError> {
        let mut stdout = io::stdout();
        if !is_raw_mode_enabled().map_err(RecordError::SetUpTerminal)? {
            enable_raw_mode().map_err(RecordError::SetUpTerminal)?;
            crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
                .map_err(RecordError::SetUpTerminal)?;
        }
        Self::install_panic_hook();
        let backend = CrosstermBackend::new(stdout);
        let mut term = Terminal::new(backend).map_err(RecordError::SetUpTerminal)?;
        let result = self.run_inner(&mut term);
        Self::clean_up_crossterm().map_err(RecordError::CleanUpTerminal)?;
        result
    }

    fn install_panic_hook() {
        // HACK: installing a global hook here. This could be installed multiple
        // times, and there's no way to uninstall it once we return.
        //
        // The idea is
        // taken from
        // https://github.com/fdehau/tui-rs/blob/fafad6c96109610825aad89c4bba5253e01101ed/examples/panic.rs.
        //
        // For some reason, simply catching the panic, cleaning up, and
        // reraising the panic loses information about where the panic was
        // originally raised, which is frustrating.
        let original_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic| {
            Self::clean_up_crossterm().unwrap();
            original_hook(panic);
        }));
    }

    fn clean_up_crossterm() -> io::Result<()> {
        if is_raw_mode_enabled()? {
            disable_raw_mode()?;
            crossterm::execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
        }
        Ok(())
    }

    fn run_testing(self, width: usize, height: usize) -> Result<RecordState<'a>, RecordError> {
        let backend = TestBackend::new(width.clamp_into_u16(), height.clamp_into_u16());
        let mut term = Terminal::new(backend).map_err(RecordError::SetUpTerminal)?;
        self.run_inner(&mut term)
    }

    fn run_inner(
        mut self,
        term: &mut Terminal<impl Backend + Any>,
    ) -> Result<RecordState<'a>, RecordError> {
        self.selection_key = self.first_selection_key();
        let debug = if cfg!(feature = "debug") {
            std::env::var_os(ENV_VAR_DEBUG_UI).is_some()
        } else {
            false
        };

        'outer: loop {
            let app = self.make_app(None);
            let term_height = usize::from(term.get_frame().size().height);

            let mut drawn_rects: Option<HashMap<ComponentId, Rect>> = None;
            term.draw(|frame| {
                drawn_rects = Some(Viewport::<ComponentId>::render_top_level(
                    frame,
                    0,
                    self.scroll_offset_y,
                    &app,
                ));
            })
            .map_err(RecordError::RenderFrame)?;
            let drawn_rects = drawn_rects.unwrap();

            // Dump debug info. We may need to use information about the
            // rendered app, so we perform a re-render here.
            if debug {
                let debug_info = AppDebugInfo {
                    term_height,
                    scroll_offset_y: self.scroll_offset_y,
                    selection_key: self.selection_key,
                    selection_key_y: self.selection_key_y(&drawn_rects, self.selection_key),
                    app_drawn_vs_expected_height: {
                        let drawn_height = drawn_rects[&ComponentId::App].height;
                        let expected_height = app.height();
                        (drawn_height, expected_height)
                    },
                    drawn_rects: drawn_rects.clone().into_iter().collect(),
                };
                let debug_app = App {
                    debug_info: Some(debug_info),
                    ..app.clone()
                };
                term.draw(|frame| {
                    Viewport::<ComponentId>::render_top_level(
                        frame,
                        0,
                        self.scroll_offset_y,
                        &debug_app,
                    );
                })
                .map_err(RecordError::RenderFrame)?;
            }

            let events = self.event_source.next_events()?;
            for event in events {
                match self.handle_event(event, term_height, &drawn_rects)? {
                    StateUpdate::None => {}
                    StateUpdate::SetQuitDialog(quit_dialog) => {
                        self.quit_dialog = quit_dialog;
                    }
                    StateUpdate::QuitAccept => break 'outer,
                    StateUpdate::QuitCancel => return Err(RecordError::Cancelled),
                    StateUpdate::TakeScreenshot(screenshot) => {
                        let backend: &dyn Any = term.backend();
                        let test_backend = backend
                            .downcast_ref::<TestBackend>()
                            .expect("TakeScreenshot event generated for non-testing backend");
                        screenshot.set(buffer_view(test_backend.buffer()));
                    }
                    StateUpdate::ScrollTo(scroll_offset_y) => {
                        self.scroll_offset_y = scroll_offset_y
                            .clamp(0, drawn_rects[&ComponentId::App].height.unwrap_isize() - 1);
                    }
                    StateUpdate::SelectItem(selection_key) => {
                        self.selection_key = selection_key;
                        self.scroll_offset_y =
                            self.ensure_in_viewport(term_height, &drawn_rects, selection_key);
                    }
                    StateUpdate::ToggleItem(selection_key) => {
                        self.toggle_item(selection_key)?;
                    }
                    StateUpdate::ToggleItemAndAdvance(selection_key, new_key) => {
                        self.toggle_item(selection_key)?;
                        self.selection_key = new_key;
                        self.scroll_offset_y =
                            self.ensure_in_viewport(term_height, &drawn_rects, selection_key);
                    }
                    StateUpdate::ToggleAll => {
                        self.toggle_all();
                    }
                }
            }
        }

        Ok(self.state)
    }

    fn make_app(&'a self, debug_info: Option<AppDebugInfo>) -> App<'a> {
        let file_views: Vec<FileView> = self
            .state
            .files
            .iter()
            .enumerate()
            .map(|(file_idx, file)| {
                let file_key = FileKey { file_idx };
                let file_tristate = self.file_tristate(file_key).unwrap();
                let is_focused = match self.selection_key {
                    SelectionKey::None | SelectionKey::Section(_) | SelectionKey::Line(_) => false,
                    SelectionKey::File(selected_file_key) => file_key == selected_file_key,
                };
                FileView {
                    debug: debug_info.is_some(),
                    file_key,
                    tristate_box: TristateBox {
                        use_unicode: self.use_unicode,
                        id: ComponentId::TristateBox(SelectionKey::File(file_key)),
                        tristate: file_tristate,
                        is_focused,
                    },
                    is_header_selected: is_focused,
                    old_path: file.old_path.as_deref(),
                    path: &file.path,
                    section_views: {
                        let mut section_views = Vec::new();
                        let total_num_sections = file.sections.len();
                        let total_num_editable_sections = file
                            .sections
                            .iter()
                            .filter(|section| section.is_editable())
                            .count();

                        let mut line_num = 1;
                        let mut editable_section_num = 0;
                        for (section_idx, section) in file.sections.iter().enumerate() {
                            let section_key = SectionKey {
                                file_idx,
                                section_idx,
                            };
                            let section_tristate = self.section_tristate(section_key).unwrap();
                            if section.is_editable() {
                                editable_section_num += 1;
                            }
                            section_views.push(SectionView {
                                use_unicode: self.use_unicode,
                                section_key,
                                tristate_box: TristateBox {
                                    use_unicode: self.use_unicode,
                                    id: ComponentId::TristateBox(SelectionKey::Section(
                                        section_key,
                                    )),
                                    tristate: section_tristate,
                                    is_focused: match self.selection_key {
                                        SelectionKey::None
                                        | SelectionKey::File(_)
                                        | SelectionKey::Line(_) => false,
                                        SelectionKey::Section(selection_section_key) => {
                                            selection_section_key == section_key
                                        }
                                    },
                                },
                                selection: match self.selection_key {
                                    SelectionKey::None | SelectionKey::File(_) => None,
                                    SelectionKey::Section(selected_section_key) => {
                                        if selected_section_key == section_key {
                                            Some(SectionSelection::SectionHeader)
                                        } else {
                                            None
                                        }
                                    }
                                    SelectionKey::Line(LineKey {
                                        file_idx,
                                        section_idx,
                                        line_idx,
                                    }) => {
                                        let selected_section_key = SectionKey {
                                            file_idx,
                                            section_idx,
                                        };
                                        if selected_section_key == section_key {
                                            Some(SectionSelection::ChangedLine(line_idx))
                                        } else {
                                            None
                                        }
                                    }
                                },
                                total_num_sections,
                                editable_section_num,
                                total_num_editable_sections,
                                section,
                                line_start_num: line_num,
                            });

                            line_num += match section {
                                Section::Unchanged { lines } => lines.len(),
                                Section::Changed { lines } => lines
                                    .iter()
                                    .filter(|changed_line| match changed_line.change_type {
                                        ChangeType::Added => false,
                                        ChangeType::Removed => true,
                                    })
                                    .count(),
                                Section::FileMode { .. } | Section::Binary { .. } => 0,
                            };
                        }
                        section_views
                    },
                }
            })
            .collect();
        App {
            debug_info: None,
            file_views,
            quit_dialog: self.quit_dialog.clone(),
        }
    }

    fn handle_event(
        &self,
        event: Event,
        term_height: usize,
        drawn_rects: &HashMap<ComponentId, Rect>,
    ) -> Result<StateUpdate, RecordError> {
        let state_update = match (&self.quit_dialog, event) {
            (_, Event::None) => StateUpdate::None,

            // Confirm the changes.
            (None, Event::QuitAccept) => StateUpdate::QuitAccept,
            // Ignore the confirm action if the quit dialog is open.
            (Some(_), Event::QuitAccept) => StateUpdate::None,

            // Render quit dialog if the user made changes.
            (None, Event::QuitCancel | Event::QuitInterrupt) => {
                let num_changed_files = self.num_user_file_changes()?;
                if num_changed_files > 0 {
                    StateUpdate::SetQuitDialog(Some(QuitDialog {
                        num_changed_files,
                        focused_button: QuitDialogButtonId::Quit,
                    }))
                } else {
                    StateUpdate::QuitCancel
                }
            }
            // If pressing quit again while the dialog is open, close it.
            (Some(_), Event::QuitCancel) => StateUpdate::SetQuitDialog(None),
            // If pressing ctrl-c again wile the dialog is open, force quit.
            (Some(_), Event::QuitInterrupt) => StateUpdate::QuitCancel,
            // Select left quit dialog button.
            (Some(quit_dialog), Event::FocusOuter) => {
                StateUpdate::SetQuitDialog(Some(QuitDialog {
                    focused_button: QuitDialogButtonId::GoBack,
                    ..quit_dialog.clone()
                }))
            }
            // Select right quit dialog button.
            (Some(quit_dialog), Event::FocusInner) => {
                StateUpdate::SetQuitDialog(Some(QuitDialog {
                    focused_button: QuitDialogButtonId::Quit,
                    ..quit_dialog.clone()
                }))
            }
            // Press the appropriate dialog button.
            (Some(quit_dialog), Event::ToggleItem | Event::ToggleItemAndAdvance) => {
                let QuitDialog {
                    num_changed_files: _,
                    focused_button,
                } = quit_dialog;
                match focused_button {
                    QuitDialogButtonId::Quit => StateUpdate::QuitCancel,
                    QuitDialogButtonId::GoBack => StateUpdate::SetQuitDialog(None),
                }
            }

            // Disable most keyboard shortcuts while the quit dialog is open.
            (
                Some(_),
                Event::ScrollUp
                | Event::ScrollDown
                | Event::PageUp
                | Event::PageDown
                | Event::FocusPrev
                | Event::FocusNext
                | Event::FocusPrevPage
                | Event::FocusNextPage
                | Event::ToggleAll,
            ) => StateUpdate::None,

            (Some(_) | None, Event::TakeScreenshot(screenshot)) => {
                StateUpdate::TakeScreenshot(screenshot)
            }
            (None, Event::ScrollUp) => {
                StateUpdate::ScrollTo(self.scroll_offset_y.saturating_sub(1))
            }
            (None, Event::ScrollDown) => {
                StateUpdate::ScrollTo(self.scroll_offset_y.saturating_add(1))
            }
            (None, Event::PageUp) => StateUpdate::ScrollTo(
                self.scroll_offset_y
                    .saturating_sub(term_height.unwrap_isize()),
            ),
            (None, Event::PageDown) => StateUpdate::ScrollTo(
                self.scroll_offset_y
                    .saturating_add(term_height.unwrap_isize()),
            ),
            (None, Event::FocusPrev) => {
                let (keys, index) = self.find_selection();
                let selection_key = self.select_prev(&keys, index);
                StateUpdate::SelectItem(selection_key)
            }
            (None, Event::FocusNext) => {
                let (keys, index) = self.find_selection();
                let selection_key = self.select_next(&keys, index);
                StateUpdate::SelectItem(selection_key)
            }
            (None, Event::FocusPrevPage) => {
                let selection_key = self.select_prev_page(term_height, drawn_rects);
                StateUpdate::SelectItem(selection_key)
            }
            (None, Event::FocusNextPage) => {
                let selection_key = self.select_next_page(term_height, drawn_rects);
                StateUpdate::SelectItem(selection_key)
            }
            (None, Event::FocusInner | Event::FocusOuter) => {
                // TODO: implement
                StateUpdate::None
            }
            (None, Event::ToggleItem) => StateUpdate::ToggleItem(self.selection_key),
            (None, Event::ToggleItemAndAdvance) => {
                let advanced_key = self.advance_to_next_of_kind();
                StateUpdate::ToggleItemAndAdvance(self.selection_key, advanced_key)
            }
            (None, Event::ToggleAll) => StateUpdate::ToggleAll,

            (_, Event::Click { row, column }) => {
                let component_id = self.find_component_at(drawn_rects, row, column);
                self.click_component(component_id)
            }
        };
        Ok(state_update)
    }

    fn first_selection_key(&self) -> SelectionKey {
        match self.state.files.iter().enumerate().next() {
            Some((file_idx, _)) => SelectionKey::File(FileKey { file_idx }),
            None => SelectionKey::None,
        }
    }

    fn num_user_file_changes(&self) -> Result<usize, RecordError> {
        let RecordState { files } = &self.state;
        let mut result = 0;
        for (file_idx, _file) in files.iter().enumerate() {
            match self.file_tristate(FileKey { file_idx })? {
                Tristate::Unchecked => {}
                Tristate::Partial | Tristate::Checked => {
                    result += 1;
                }
            }
        }
        Ok(result)
    }

    fn all_selection_keys(&self) -> Vec<SelectionKey> {
        let mut result = Vec::new();
        for (file_idx, file) in self.state.files.iter().enumerate() {
            result.push(SelectionKey::File(FileKey { file_idx }));
            for (section_idx, section) in file.sections.iter().enumerate() {
                match section {
                    Section::Unchanged { .. } => {}
                    Section::Changed { lines } => {
                        result.push(SelectionKey::Section(SectionKey {
                            file_idx,
                            section_idx,
                        }));
                        for (line_idx, _line) in lines.iter().enumerate() {
                            result.push(SelectionKey::Line(LineKey {
                                file_idx,
                                section_idx,
                                line_idx,
                            }));
                        }
                    }
                    Section::FileMode {
                        is_toggled: _,
                        before: _,
                        after: _,
                    }
                    | Section::Binary { .. } => {
                        result.push(SelectionKey::Section(SectionKey {
                            file_idx,
                            section_idx,
                        }));
                    }
                }
            }
        }
        result
    }

    fn find_selection(&self) -> (Vec<SelectionKey>, Option<usize>) {
        // FIXME: O(n) algorithm
        let keys = self.all_selection_keys();
        let index = keys.iter().enumerate().find_map(|(k, v)| {
            if v == &self.selection_key {
                Some(k)
            } else {
                None
            }
        });
        (keys, index)
    }

    fn select_prev(&self, keys: &[SelectionKey], index: Option<usize>) -> SelectionKey {
        match index {
            None => self.first_selection_key(),
            Some(index) => match index.checked_sub(1) {
                Some(index) => keys[index],
                None => *keys.last().unwrap(),
            },
        }
    }

    fn select_next(&self, keys: &[SelectionKey], index: Option<usize>) -> SelectionKey {
        match index {
            None => self.first_selection_key(),
            Some(index) => match keys.get(index + 1) {
                Some(key) => *key,
                None => keys[0],
            },
        }
    }

    fn select_prev_page(
        &self,
        term_height: usize,
        drawn_rects: &HashMap<ComponentId, Rect>,
    ) -> SelectionKey {
        let (keys, index) = self.find_selection();
        let mut index = match index {
            Some(index) => index,
            None => return SelectionKey::None,
        };

        let original_y = self.selection_key_y(drawn_rects, self.selection_key);
        let target_y = original_y.saturating_sub(term_height.unwrap_isize() / 2);
        while index > 0 {
            index -= 1;
            if self.selection_key_y(drawn_rects, keys[index]) <= target_y {
                break;
            }
        }
        keys[index]
    }

    fn select_next_page(
        &self,
        term_height: usize,
        drawn_rects: &HashMap<ComponentId, Rect>,
    ) -> SelectionKey {
        let (keys, index) = self.find_selection();
        let mut index = match index {
            Some(index) => index,
            None => return SelectionKey::None,
        };

        let original_y = self.selection_key_y(drawn_rects, self.selection_key);
        let target_y = original_y.saturating_add(term_height.unwrap_isize() / 2);
        while index + 1 < keys.len() {
            index += 1;
            if self.selection_key_y(drawn_rects, keys[index]) >= target_y {
                break;
            }
        }
        keys[index]
    }

    fn advance_to_next_of_kind(&self) -> SelectionKey {
        let (keys, index) = self.find_selection();
        let index = match index {
            Some(index) => index,
            None => return SelectionKey::None,
        };
        keys.iter()
            .skip(index + 1)
            .copied()
            .find(|key| match (self.selection_key, key) {
                (SelectionKey::None, _)
                | (SelectionKey::File(_), SelectionKey::File(_))
                | (SelectionKey::Section(_), SelectionKey::Section(_))
                | (SelectionKey::Line(_), SelectionKey::Line(_)) => true,
                (
                    SelectionKey::File(_),
                    SelectionKey::None | SelectionKey::Section(_) | SelectionKey::Line(_),
                )
                | (
                    SelectionKey::Section(_),
                    SelectionKey::None | SelectionKey::File(_) | SelectionKey::Line(_),
                )
                | (
                    SelectionKey::Line(_),
                    SelectionKey::None | SelectionKey::File(_) | SelectionKey::Section(_),
                ) => false,
            })
            .unwrap_or(self.selection_key)
    }

    fn selection_key_y(
        &self,
        drawn_rects: &HashMap<ComponentId, Rect>,
        selection_key: SelectionKey,
    ) -> isize {
        let rect = self.selection_rect(drawn_rects, selection_key);
        rect.y
    }

    fn selection_rect(
        &self,
        drawn_rects: &HashMap<ComponentId, Rect>,
        selection_key: SelectionKey,
    ) -> Rect {
        let id = ComponentId::SelectableItem(selection_key);
        match drawn_rects.get(&id) {
            Some(drawn_rect) => *drawn_rect,
            None => {
                panic!("could not look up drawn rect for component with ID {id:?}; was it drawn?")
            }
        }
    }

    fn ensure_in_viewport(
        &self,
        term_height: usize,
        drawn_rects: &HashMap<ComponentId, Rect>,
        selection_key: SelectionKey,
    ) -> isize {
        // Idea: scroll the entire component into the viewport, not just the
        // first line, if possible. If the entire component is smaller than
        // the viewport, then we scroll only enough so that the entire
        // component becomes visible, i.e. align the component's bottom edge
        // with the viewport's bottom edge. Otherwise, we scroll such that
        // the component's top edge is aligned with the viewport's top edge.
        //
        // FIXME: if we scroll up from below, we would want to align the top
        // edge of the component, not the bottom edge. Thus, we should also
        // accept the previous `SelectionKey` and use that when making the
        // decision of where to scroll.
        let term_height = term_height.unwrap_isize();
        let rect = self.selection_rect(drawn_rects, selection_key);
        let rect_height = rect.height.unwrap_isize();
        let rect_bottom_y = rect.y + rect_height;
        if self.scroll_offset_y <= rect.y && rect_bottom_y < self.scroll_offset_y + term_height {
            // Component is completely within the viewport, no need to scroll.
            self.scroll_offset_y
        } else if (
            // Component doesn't fit in the viewport; just render the top.
            rect_height >= term_height
        ) || (
            // Component is at least partially above the viewport.
            rect.y < self.scroll_offset_y
        ) {
            rect.y
        } else {
            // Component is at least partially below the viewport. Want to satisfy:
            // scroll_offset_y + term_height == rect_bottom_y
            rect_bottom_y - term_height
        }
    }

    fn find_component_at(
        &self,
        drawn_rects: &HashMap<ComponentId, Rect>,
        row: usize,
        column: usize,
    ) -> ComponentId {
        let x = column.unwrap_isize();
        let y = row.unwrap_isize() + self.scroll_offset_y;
        drawn_rects
            .iter()
            .filter(|(id, rect)| {
                rect.contains_point(x, y)
                    && match id {
                        ComponentId::App => false,
                        ComponentId::SelectableItem(_)
                        | ComponentId::TristateBox(_)
                        | ComponentId::QuitDialog
                        | ComponentId::QuitDialogButton(_) => true,
                    }
            })
            .min_by_key(|(id, rect)| (rect.size().area(), (rect.y, rect.x), *id))
            .map(|(id, _rect)| *id)
            .unwrap_or(ComponentId::App)
    }

    fn click_component(&self, component_id: ComponentId) -> StateUpdate {
        match component_id {
            ComponentId::App | ComponentId::QuitDialog => StateUpdate::None,
            ComponentId::SelectableItem(selection_key) => StateUpdate::SelectItem(selection_key),
            ComponentId::TristateBox(selection_key) => {
                if self.selection_key == selection_key {
                    StateUpdate::ToggleItem(selection_key)
                } else {
                    StateUpdate::SelectItem(selection_key)
                }
            }
            ComponentId::QuitDialogButton(QuitDialogButtonId::GoBack) => {
                StateUpdate::SetQuitDialog(None)
            }
            ComponentId::QuitDialogButton(QuitDialogButtonId::Quit) => StateUpdate::QuitCancel,
        }
    }

    fn toggle_item(&mut self, selection: SelectionKey) -> Result<(), RecordError> {
        match selection {
            SelectionKey::None => {}
            SelectionKey::File(file_key) => {
                let tristate = self.file_tristate(file_key)?;
                let is_toggled_new = match tristate {
                    Tristate::Unchecked => true,
                    Tristate::Partial | Tristate::Checked => false,
                };
                self.visit_file(file_key, |file| {
                    for section in file.sections.iter_mut() {
                        match section {
                            Section::Unchanged { .. } => {}
                            Section::Changed { lines } => {
                                for line in lines {
                                    line.is_toggled = is_toggled_new;
                                }
                            }
                            Section::FileMode {
                                is_toggled,
                                before: _,
                                after: _,
                            }
                            | Section::Binary {
                                is_toggled,
                                old_description: _,
                                new_description: _,
                            } => {
                                *is_toggled = is_toggled_new;
                            }
                        }
                    }
                })?;
            }
            SelectionKey::Section(section_key) => {
                let tristate = self.section_tristate(section_key)?;
                let is_focused_new = match tristate {
                    Tristate::Unchecked => true,
                    Tristate::Partial | Tristate::Checked => false,
                };
                self.visit_section(section_key, |section| match section {
                    Section::Unchanged { .. } => {}
                    Section::Changed { lines } => {
                        for line in lines {
                            line.is_toggled = is_focused_new;
                        }
                    }
                    Section::FileMode {
                        is_toggled,
                        before: _,
                        after: _,
                    }
                    | Section::Binary {
                        is_toggled,
                        old_description: _,
                        new_description: _,
                    } => {
                        *is_toggled = is_focused_new;
                    }
                })?;
            }
            SelectionKey::Line(line_key) => {
                self.visit_line(line_key, |line| {
                    line.is_toggled = !line.is_toggled;
                })?;
            }
        }
        Ok(())
    }

    fn toggle_all(&mut self) {
        for file in &mut self.state.files {
            for section in &mut file.sections {
                section.toggle_all();
            }
        }
    }

    fn file(&self, file_key: FileKey) -> Result<&File, RecordError> {
        let FileKey { file_idx } = file_key;
        match self.state.files.get(file_idx) {
            Some(file) => Ok(file),
            None => Err(RecordError::Bug(format!(
                "Out-of-bounds file key: {file_key:?}"
            ))),
        }
    }

    fn section(&self, section_key: SectionKey) -> Result<&Section, RecordError> {
        let SectionKey {
            file_idx,
            section_idx,
        } = section_key;
        let file = self.file(FileKey { file_idx })?;
        match file.sections.get(section_idx) {
            Some(section) => Ok(section),
            None => Err(RecordError::Bug(format!(
                "Out-of-bounds section key: {section_key:?}"
            ))),
        }
    }

    fn visit_file<T>(
        &mut self,
        file_key: FileKey,
        f: impl Fn(&mut File) -> T,
    ) -> Result<T, RecordError> {
        let FileKey { file_idx } = file_key;
        match self.state.files.get_mut(file_idx) {
            Some(file) => Ok(f(file)),
            None => Err(RecordError::Bug(format!(
                "Out-of-bounds file key: {file_key:?}"
            ))),
        }
    }

    fn file_tristate(&self, file_key: FileKey) -> Result<Tristate, RecordError> {
        let mut seen_value = None;
        for section in &self.file(file_key)?.sections {
            match section {
                Section::Unchanged { .. } => {}
                Section::Changed { lines } => {
                    for line in lines {
                        seen_value = match (seen_value, line.is_toggled) {
                            (None, is_focused) => Some(is_focused),
                            (Some(true), true) => Some(true),
                            (Some(false), false) => Some(false),
                            (Some(true), false) | (Some(false), true) => {
                                return Ok(Tristate::Partial)
                            }
                        };
                    }
                }
                Section::FileMode {
                    is_toggled,
                    before: _,
                    after: _,
                }
                | Section::Binary {
                    is_toggled,
                    old_description: _,
                    new_description: _,
                } => {
                    seen_value = match (seen_value, is_toggled) {
                        (None, is_focused) => Some(*is_focused),
                        (Some(true), true) => Some(true),
                        (Some(false), false) => Some(false),
                        (Some(true), false) | (Some(false), true) => return Ok(Tristate::Partial),
                    }
                }
            }
        }
        let result = match seen_value {
            Some(true) => Tristate::Checked,
            None | Some(false) => Tristate::Unchecked,
        };
        Ok(result)
    }

    fn visit_section<T>(
        &mut self,
        section_key: SectionKey,
        f: impl Fn(&mut Section) -> T,
    ) -> Result<T, RecordError> {
        let SectionKey {
            file_idx,
            section_idx,
        } = section_key;
        let file = match self.state.files.get_mut(file_idx) {
            Some(file) => file,
            None => {
                return Err(RecordError::Bug(format!(
                    "Out-of-bounds file for section key: {section_key:?}"
                )))
            }
        };
        match file.sections.get_mut(section_idx) {
            Some(section) => Ok(f(section)),
            None => Err(RecordError::Bug(format!(
                "Out-of-bounds section key: {section_key:?}"
            ))),
        }
    }

    fn section_tristate(&self, section_key: SectionKey) -> Result<Tristate, RecordError> {
        let mut seen_value = None;
        match self.section(section_key)? {
            Section::Unchanged { .. } => {}
            Section::Changed { lines } => {
                for line in lines {
                    seen_value = match (seen_value, line.is_toggled) {
                        (None, is_focused) => Some(is_focused),
                        (Some(true), true) => Some(true),
                        (Some(false), false) => Some(false),
                        (Some(true), false) | (Some(false), true) => return Ok(Tristate::Partial),
                    };
                }
            }
            Section::FileMode {
                is_toggled,
                before: _,
                after: _,
            }
            | Section::Binary {
                is_toggled,
                old_description: _,
                new_description: _,
            } => {
                seen_value = match (seen_value, is_toggled) {
                    (None, is_toggled) => Some(*is_toggled),
                    (Some(true), true) => Some(true),
                    (Some(false), false) => Some(false),
                    (Some(true), false) | (Some(false), true) => return Ok(Tristate::Partial),
                }
            }
        }
        let result = match seen_value {
            Some(true) => Tristate::Checked,
            None | Some(false) => Tristate::Unchecked,
        };
        Ok(result)
    }

    fn visit_line<T>(
        &mut self,
        line_key: LineKey,
        f: impl FnOnce(&mut SectionChangedLine) -> T,
    ) -> Result<T, RecordError> {
        let LineKey {
            file_idx,
            section_idx,
            line_idx,
        } = line_key;
        let section = &mut self.state.files[file_idx].sections[section_idx];
        match section {
            Section::Changed { lines } => {
                let line = &mut lines[line_idx];
                Ok(f(line))
            }
            section @ (Section::Unchanged { .. }
            | Section::FileMode { .. }
            | Section::Binary { .. }) => Err(RecordError::Bug(format!(
                "Bad line key {line_key:?}, tried to index section {section:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
enum ComponentId {
    App,
    SelectableItem(SelectionKey),
    TristateBox(SelectionKey),
    QuitDialog,
    QuitDialogButton(QuitDialogButtonId),
}

#[derive(Clone, Debug)]
enum Tristate {
    Unchecked,
    Partial,
    Checked,
}

impl From<bool> for Tristate {
    fn from(value: bool) -> Self {
        if value {
            Tristate::Checked
        } else {
            Tristate::Unchecked
        }
    }
}

#[derive(Clone, Debug)]
struct TristateBox<Id> {
    use_unicode: bool,
    id: Id,
    tristate: Tristate,
    is_focused: bool,
}

impl<Id> TristateBox<Id> {
    fn text(&self) -> &'static str {
        let Self {
            use_unicode,
            id: _,
            tristate,
            is_focused,
        } = self;

        match (tristate, is_focused, use_unicode) {
            (Tristate::Unchecked, false, _) => "[ ]",
            (Tristate::Unchecked, true, _) => "( )",
            (Tristate::Partial, false, _) => "[~]",
            (Tristate::Partial, true, _) => "(~)",
            (Tristate::Checked, false, false) => "[x]",
            (Tristate::Checked, true, false) => "(x)",
            (Tristate::Checked, false, true) => "[\u{00D7}]", // Multiplication Sign
            (Tristate::Checked, true, true) => "(\u{00D7})",  // Multiplication Sign
        }
    }
}

impl<Id: Clone + Debug + Eq + Hash> Component for TristateBox<Id> {
    type Id = Id;

    fn id(&self) -> Self::Id {
        self.id.clone()
    }

    fn draw(&self, viewport: &mut Viewport<Self::Id>, x: isize, y: isize) {
        let span = Span::styled(self.text(), Style::default().add_modifier(Modifier::BOLD));
        viewport.draw_span(x, y, &span);
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
struct AppDebugInfo {
    term_height: usize,
    scroll_offset_y: isize,
    selection_key: SelectionKey,
    selection_key_y: isize,
    app_drawn_vs_expected_height: (usize, usize),
    drawn_rects: BTreeMap<ComponentId, Rect>, // sorted for determinism
}

#[derive(Clone, Debug)]
struct App<'a> {
    debug_info: Option<AppDebugInfo>,
    file_views: Vec<FileView<'a>>,
    quit_dialog: Option<QuitDialog>,
}

impl App<'_> {
    fn height(&self) -> usize {
        let Self {
            debug_info: _,
            file_views,
            quit_dialog: _,
        } = self;
        file_views.iter().map(|file_view| file_view.height()).sum()
    }
}

impl Component for App<'_> {
    type Id = ComponentId;

    fn id(&self) -> Self::Id {
        ComponentId::App
    }

    fn draw(&self, viewport: &mut Viewport<Self::Id>, x: isize, y: isize) {
        let Self {
            debug_info,
            file_views,
            quit_dialog,
        } = self;

        if let Some(debug_info) = debug_info {
            viewport.debug(format!("app debug info: {debug_info:#?}"));
        }

        let mut y = y;
        for file_view in file_views {
            let file_view_rect = viewport.draw_component(x, y, file_view);
            y += file_view_rect.height.unwrap_isize();

            if debug_info.is_some() {
                viewport.debug(format!(
                    "file {} dims: {file_view_rect:?}",
                    file_view.path.to_string_lossy()
                ));
            }
        }

        if let Some(quit_dialog) = quit_dialog {
            viewport.draw_component(0, 0, quit_dialog);
        }
    }
}

#[derive(Clone, Debug)]
struct FileView<'a> {
    debug: bool,
    file_key: FileKey,
    tristate_box: TristateBox<ComponentId>,
    is_header_selected: bool,
    old_path: Option<&'a Path>,
    path: &'a Path,
    section_views: Vec<SectionView<'a>>,
}

impl FileView<'_> {
    pub fn height(&self) -> usize {
        1 + self
            .section_views
            .iter()
            .map(|section_view| section_view.height())
            .sum::<usize>()
    }
}

impl Component for FileView<'_> {
    type Id = ComponentId;

    fn id(&self) -> Self::Id {
        ComponentId::SelectableItem(SelectionKey::File(self.file_key))
    }

    fn draw(&self, viewport: &mut Viewport<Self::Id>, x: isize, y: isize) {
        let Self {
            debug,
            file_key: _,
            tristate_box,
            old_path,
            path,
            section_views,
            is_header_selected,
        } = self;
        viewport.fill_rest_of_line(x, y);

        let tristate_box_rect = viewport.draw_component(x, y, tristate_box);
        viewport.draw_span(
            x + tristate_box_rect.width.unwrap_isize() + 1,
            y,
            &Span::styled(
                format!(
                    "{}{}",
                    match old_path {
                        Some(old_path) => format!("{} => ", old_path.to_string_lossy()),
                        None => String::new(),
                    },
                    path.to_string_lossy(),
                ),
                if *is_header_selected {
                    Style::default().fg(Color::Blue)
                } else {
                    Style::default()
                },
            ),
        );
        if *is_header_selected {
            highlight_line(viewport, y);
        }

        let x = x + 2;
        let mut y = y + 1;
        for section_view in section_views {
            let section_rect = viewport.draw_component(x, y, section_view);
            y += section_rect.height.unwrap_isize();

            if *debug {
                viewport.debug(format!("section dims: {section_rect:?}",));
            }
        }
    }
}

#[derive(Clone, Debug)]
enum SectionSelection {
    SectionHeader,
    ChangedLine(usize),
}

#[derive(Clone, Debug)]
struct SectionView<'a> {
    use_unicode: bool,
    section_key: SectionKey,
    tristate_box: TristateBox<ComponentId>,
    selection: Option<SectionSelection>,
    total_num_sections: usize,
    editable_section_num: usize,
    total_num_editable_sections: usize,
    section: &'a Section<'a>,
    line_start_num: usize,
}

impl SectionView<'_> {
    pub fn height(&self) -> usize {
        match self.section {
            Section::Unchanged { lines } => lines.len().min(NUM_CONTEXT_LINES * 2 + 1),
            Section::Changed { lines } => lines.len() + 1,
            Section::FileMode { .. } | Section::Binary { .. } => 1,
        }
    }
}

impl Component for SectionView<'_> {
    type Id = ComponentId;

    fn id(&self) -> Self::Id {
        ComponentId::SelectableItem(SelectionKey::Section(self.section_key))
    }

    fn draw(&self, viewport: &mut Viewport<Self::Id>, x: isize, y: isize) {
        let Self {
            use_unicode,
            section_key,
            tristate_box,
            selection,
            total_num_sections,
            editable_section_num,
            total_num_editable_sections,
            section,
            line_start_num,
        } = self;
        viewport.fill_rest_of_line(x, y);

        let SectionKey {
            file_idx,
            section_idx,
        } = *section_key;
        match section {
            Section::Unchanged { lines } => {
                if lines.is_empty() {
                    return;
                }

                let lines: Vec<_> = lines.iter().enumerate().collect();
                let is_first_section = section_idx == 0;
                let is_last_section = section_idx + 1 == *total_num_sections;
                let before_ellipsis_lines = &lines[..min(NUM_CONTEXT_LINES, lines.len())];
                let after_ellipsis_lines = &lines[lines.len().saturating_sub(NUM_CONTEXT_LINES)..];

                match (before_ellipsis_lines, after_ellipsis_lines) {
                    ([.., (last_before_idx, _)], [(first_after_idx, _), ..])
                        if *last_before_idx + 1 >= *first_after_idx
                            && !is_first_section
                            && !is_last_section =>
                    {
                        let first_before_idx = before_ellipsis_lines.first().unwrap().0;
                        let last_after_idx = after_ellipsis_lines.last().unwrap().0;
                        let overlapped_lines = &lines[first_before_idx..=last_after_idx];
                        let overlapped_lines = if is_first_section {
                            &overlapped_lines
                                [overlapped_lines.len().saturating_sub(NUM_CONTEXT_LINES)..]
                        } else if is_last_section {
                            &overlapped_lines[..lines.len().min(NUM_CONTEXT_LINES)]
                        } else {
                            overlapped_lines
                        };
                        for (dy, (line_idx, line)) in overlapped_lines.iter().enumerate() {
                            let line_view = SectionLineView {
                                line_key: LineKey {
                                    file_idx,
                                    section_idx,
                                    line_idx: *line_idx,
                                },
                                inner: SectionLineViewInner::Unchanged {
                                    line: line.as_ref(),
                                    line_num: line_start_num + line_idx,
                                },
                            };
                            viewport.draw_component(x + 2, y + dy.unwrap_isize(), &line_view);
                        }
                        return;
                    }
                    _ => {}
                };

                let mut dy = 0;
                if !is_first_section {
                    for (line_idx, line) in before_ellipsis_lines {
                        let line_view = SectionLineView {
                            line_key: LineKey {
                                file_idx,
                                section_idx,
                                line_idx: *line_idx,
                            },
                            inner: SectionLineViewInner::Unchanged {
                                line: line.as_ref(),
                                line_num: line_start_num + line_idx,
                            },
                        };
                        viewport.draw_component(x + 2, y + dy, &line_view);
                        dy += 1;
                    }
                }

                let should_render_ellipsis = lines.len() > NUM_CONTEXT_LINES;
                if should_render_ellipsis {
                    let ellipsis = if *use_unicode {
                        "\u{22EE}" // Vertical Ellipsis
                    } else {
                        ":"
                    };
                    viewport.draw_span(
                        x + 6, // align with line numbering
                        y + dy,
                        &Span::styled(ellipsis, Style::default().add_modifier(Modifier::DIM)),
                    );
                    dy += 1;
                }

                if !is_last_section {
                    for (line_idx, line) in after_ellipsis_lines {
                        let line_view = SectionLineView {
                            line_key: LineKey {
                                file_idx,
                                section_idx,
                                line_idx: *line_idx,
                            },
                            inner: SectionLineViewInner::Unchanged {
                                line: line.as_ref(),
                                line_num: line_start_num + line_idx,
                            },
                        };
                        viewport.draw_component(x + 2, y + dy, &line_view);
                        dy += 1;
                    }
                }
            }

            Section::Changed { lines } => {
                // Draw section header.
                let tristate_rect = viewport.draw_component(x, y, tristate_box);
                viewport.draw_span(
                    x + tristate_rect.width.unwrap_isize() + 1,
                    y,
                    &Span::styled(
                        format!("Section {editable_section_num}/{total_num_editable_sections}"),
                        Style::default(),
                    ),
                );
                match selection {
                    Some(SectionSelection::SectionHeader) => highlight_line(viewport, y),
                    Some(SectionSelection::ChangedLine(_)) | None => {}
                }
                let y = y + 1;

                // Draw changed lines.
                for (line_idx, line) in lines.iter().enumerate() {
                    let SectionChangedLine {
                        is_toggled,
                        change_type,
                        line,
                    } = line;
                    let is_focused = match selection {
                        Some(SectionSelection::ChangedLine(selected_line_idx)) => {
                            line_idx == *selected_line_idx
                        }
                        Some(SectionSelection::SectionHeader) | None => false,
                    };
                    let line_key = LineKey {
                        file_idx,
                        section_idx,
                        line_idx,
                    };
                    let tristate_box = TristateBox {
                        use_unicode: *use_unicode,
                        id: ComponentId::TristateBox(SelectionKey::Line(line_key)),
                        tristate: Tristate::from(*is_toggled),
                        is_focused,
                    };
                    let line_view = SectionLineView {
                        line_key,
                        inner: SectionLineViewInner::Changed {
                            tristate_box,
                            change_type: *change_type,
                            line: line.as_ref(),
                        },
                    };
                    let y = y + line_idx.unwrap_isize();
                    viewport.draw_component(x + 2, y, &line_view);
                    if is_focused {
                        highlight_line(viewport, y);
                    }
                }
            }

            Section::FileMode {
                is_toggled,
                before,
                after,
            } => {
                let is_focused = match selection {
                    Some(SectionSelection::SectionHeader) => true,
                    Some(SectionSelection::ChangedLine(_)) | None => false,
                };
                let section_key = SectionKey {
                    file_idx,
                    section_idx,
                };
                let selection_key = SelectionKey::Section(section_key);
                let tristate_box = TristateBox {
                    use_unicode: *use_unicode,
                    id: ComponentId::TristateBox(selection_key),
                    tristate: Tristate::from(*is_toggled),
                    is_focused,
                };
                let tristate_rect = viewport.draw_component(x, y, &tristate_box);
                let x = x + tristate_rect.width.unwrap_isize() + 1;
                let text = format!("File mode changed from {before} to {after}");
                viewport.draw_span(x, y, &Span::styled(text, Style::default().fg(Color::Blue)));
                if is_focused {
                    highlight_line(viewport, y);
                }
            }

            Section::Binary {
                is_toggled,
                old_description,
                new_description,
            } => {
                let is_focused = match selection {
                    Some(SectionSelection::SectionHeader) => true,
                    Some(SectionSelection::ChangedLine(_)) | None => false,
                };
                let section_key = SectionKey {
                    file_idx,
                    section_idx,
                };
                let tristate_box = TristateBox {
                    use_unicode: *use_unicode,
                    id: ComponentId::TristateBox(SelectionKey::Section(section_key)),
                    tristate: Tristate::from(*is_toggled),
                    is_focused,
                };
                let tristate_rect = viewport.draw_component(x, y, &tristate_box);
                let x = x + tristate_rect.width.unwrap_isize() + 1;

                let text = {
                    let mut result =
                        vec![if old_description.is_some() || new_description.is_some() {
                            "binary contents:"
                        } else {
                            "binary contents"
                        }
                        .to_string()];
                    let description: Vec<_> = [old_description, new_description]
                        .iter()
                        .copied()
                        .flatten()
                        .map(|s| s.as_ref())
                        .collect();
                    result.push(description.join(" -> "));
                    format!("({})", result.join(" "))
                };
                viewport.draw_span(x, y, &Span::styled(text, Style::default().fg(Color::Blue)));

                if is_focused {
                    highlight_line(viewport, y);
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
enum SectionLineViewInner<'a> {
    Unchanged {
        line: &'a str,
        line_num: usize,
    },
    Changed {
        tristate_box: TristateBox<ComponentId>,
        change_type: ChangeType,
        line: &'a str,
    },
}

#[derive(Clone, Debug)]
struct SectionLineView<'a> {
    line_key: LineKey,
    inner: SectionLineViewInner<'a>,
}

impl Component for SectionLineView<'_> {
    type Id = ComponentId;

    fn id(&self) -> Self::Id {
        ComponentId::SelectableItem(SelectionKey::Line(self.line_key))
    }

    fn draw(&self, viewport: &mut Viewport<Self::Id>, x: isize, y: isize) {
        let Self { line_key: _, inner } = self;
        viewport.fill_rest_of_line(x, y);
        match inner {
            SectionLineViewInner::Unchanged { line, line_num } => {
                let style = Style::default().add_modifier(Modifier::DIM);
                // Pad the number in 5 columns because that will align the
                // beginning of the actual text with the `+`/`-` of the changed
                // lines.
                let span = Span::styled(format!("{line_num:5} "), style);
                let line_num_rect = viewport.draw_span(x, y, &span);
                let span = Span::styled(*line, style);
                viewport.draw_span(
                    line_num_rect.x + line_num_rect.width.unwrap_isize(),
                    line_num_rect.y,
                    &span,
                );
            }

            SectionLineViewInner::Changed {
                tristate_box,
                change_type,
                line,
            } => {
                let tristate_rect = viewport.draw_component(x, y, tristate_box);
                let x = x + tristate_rect.width.unwrap_isize() + 1;

                let (change_type_text, style) = match change_type {
                    ChangeType::Added => ("+ ", Style::default().fg(Color::Green)),
                    ChangeType::Removed => ("- ", Style::default().fg(Color::Red)),
                };
                viewport.draw_span(x, y, &Span::styled(change_type_text, style));
                let x = x + change_type_text.width().unwrap_isize();
                viewport.draw_span(x, y, &Span::styled(*line, style));
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QuitDialog {
    num_changed_files: usize,
    focused_button: QuitDialogButtonId,
}

impl Component for QuitDialog {
    type Id = ComponentId;

    fn id(&self) -> Self::Id {
        ComponentId::QuitDialog
    }

    fn draw(&self, viewport: &mut Viewport<Self::Id>, _x: isize, _y: isize) {
        let Self {
            num_changed_files,
            focused_button,
        } = self;
        let title = "Quit";
        let body = format!(
            "You have changes to {num_changed_files} {}. Are you sure you want to quit?",
            if *num_changed_files == 1 {
                "file"
            } else {
                "files"
            }
        );

        let quit_button = Button {
            id: ComponentId::QuitDialogButton(QuitDialogButtonId::Quit),
            label: Cow::Borrowed("Quit"),
            is_focused: match focused_button {
                QuitDialogButtonId::Quit => true,
                QuitDialogButtonId::GoBack => false,
            },
        };
        let go_back_button = Button {
            id: ComponentId::QuitDialogButton(QuitDialogButtonId::GoBack),
            label: Cow::Borrowed("Go Back"),
            is_focused: match focused_button {
                QuitDialogButtonId::GoBack => true,
                QuitDialogButtonId::Quit => false,
            },
        };
        let buttons = [quit_button, go_back_button];

        let dialog = Dialog {
            id: ComponentId::QuitDialog,
            title: Cow::Borrowed(title),
            body: Cow::Owned(body),
            buttons: &buttons,
        };
        viewport.draw_component(0, 0, &dialog);
    }
}

struct Button<'a, Id> {
    id: Id,
    label: Cow<'a, str>,
    is_focused: bool,
}

impl<'a, Id> Button<'a, Id> {
    fn span(&self) -> Span {
        let Self {
            id: _,
            label,
            is_focused,
        } = self;
        if *is_focused {
            Span::styled(
                format!("({label})"),
                Style::default().add_modifier(Modifier::REVERSED),
            )
        } else {
            Span::styled(format!("[{label}]"), Style::default())
        }
    }

    fn width(&self) -> usize {
        self.span().width()
    }
}

impl<Id: Clone + Debug + Eq + Hash> Component for Button<'_, Id> {
    type Id = Id;

    fn id(&self) -> Self::Id {
        self.id.clone()
    }

    fn draw(&self, viewport: &mut Viewport<Self::Id>, x: isize, y: isize) {
        let span = self.span();
        viewport.draw_span(x, y, &span);
    }
}

struct Dialog<'a, Id> {
    id: Id,
    title: Cow<'a, str>,
    body: Cow<'a, str>,
    buttons: &'a [Button<'a, Id>],
}

impl<Id: Clone + Debug + Eq + Hash> Component for Dialog<'_, Id> {
    type Id = Id;

    fn id(&self) -> Self::Id {
        self.id.clone()
    }

    fn draw(&self, viewport: &mut Viewport<Self::Id>, _x: isize, _y: isize) {
        let Self {
            id: _,
            title,
            body,
            buttons,
        } = self;
        let rect = {
            let border_size = 2;
            let rect = centered_rect(
                viewport.rect(),
                RectSize {
                    // FIXME: we might want to limit the width of the text and
                    // let `Paragraph` wrap it.
                    width: body.width() + border_size,
                    height: 1 + border_size,
                },
                60,
                20,
            );

            let paragraph = Paragraph::new(body.as_ref()).block(
                Block::default()
                    .title(title.as_ref())
                    .borders(Borders::all()),
            );
            let tui_rect = viewport.translate_rect(rect);
            viewport.draw_widget(tui_rect, Clear);
            viewport.draw_widget(tui_rect, paragraph);

            rect
        };

        let mut bottom_x = rect.x + rect.width.unwrap_isize() - 1;
        let bottom_y = rect.y + rect.height.unwrap_isize() - 1;
        for button in buttons.iter() {
            bottom_x -= button.width().unwrap_isize();
            let button_rect = viewport.draw_component(bottom_x, bottom_y, button);
            bottom_x = button_rect.x - 1;
        }
    }
}

fn highlight_line<Id: Clone + Debug + Eq + Hash>(viewport: &mut Viewport<Id>, y: isize) {
    viewport.set_style(
        Rect {
            x: 0,
            y,
            width: viewport.size().width,
            height: 1,
        },
        Style::default().add_modifier(Modifier::REVERSED),
    );
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    use assert_matches::assert_matches;

    #[test]
    fn test_event_source_testing() {
        let mut event_source = EventSource::testing(80, 24, [Event::QuitCancel]);
        assert_matches!(
            event_source.next_events().unwrap().as_slice(),
            &[Event::QuitCancel]
        );
        assert_matches!(
            event_source.next_events().unwrap().as_slice(),
            &[Event::None]
        );
    }

    #[test]
    fn test_quit_returns_error() {
        let state = RecordState::default();
        let event_source = EventSource::testing(80, 24, [Event::QuitCancel]);
        let recorder = Recorder::new(state, event_source);
        assert_matches!(recorder.run(), Err(RecordError::Cancelled));

        let state = RecordState {
            files: vec![File {
                old_path: None,
                path: Cow::Borrowed(Path::new("foo/bar")),
                file_mode: None,
                sections: Default::default(),
            }],
        };
        let event_source = EventSource::testing(80, 24, [Event::QuitAccept]);
        let recorder = Recorder::new(state.clone(), event_source);
        assert_eq!(recorder.run().unwrap(), state);
    }
}
