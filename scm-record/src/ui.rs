//! UI implementation.

use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::{io, panic};

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseEvent, MouseEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, is_raw_mode_enabled, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use tracing::warn;
use tui::layout::{Constraint, Direction, Layout};
use tui::style::{Color, Modifier, Style};
use tui::text::Span;
use tui::{backend::CrosstermBackend, Terminal};

use crate::render::{Component, Rect, TopLevelComponentWidget, Viewport};
use crate::types::{ChangeType, RecordError, RecordState};
use crate::util::UsizeExt;
use crate::{File, Section, SectionChangedLine};

type Backend = CrosstermBackend<io::Stdout>;
type CrosstermTerminal = Terminal<Backend>;

#[derive(Clone, Copy, Debug)]
struct FileKey {
    file_idx: usize,
}

#[allow(dead_code)] // TODO: remove
#[derive(Clone, Copy, Debug)]
struct SectionKey {
    file_idx: usize,
    section_idx: usize,
}

#[derive(Clone, Copy, Debug)]
struct LineKey {
    file_idx: usize,
    section_idx: usize,
    line_idx: usize,
}

#[allow(dead_code)] // TODO: remove
#[derive(Clone, Copy, Debug)]
enum SelectionKey {
    None,
    File(FileKey),
    Section(SectionKey),
    Line(LineKey),
}

/// UI component to record the user's changes.
pub struct Recorder<'a> {
    state: RecordState<'a>,
    use_unicode: bool,
    selection: SelectionKey,
}

impl<'a> Recorder<'a> {
    /// Run the terminal user interface and have the user interactively select
    /// changes.
    pub fn run(state: RecordState<'a>) -> Result<RecordState<'a>, RecordError> {
        let mut stdout = io::stdout();
        if !is_raw_mode_enabled().map_err(RecordError::SetUpTerminal)? {
            enable_raw_mode().map_err(RecordError::SetUpTerminal)?;
            crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
                .map_err(RecordError::SetUpTerminal)?;
        }
        let backend = CrosstermBackend::new(stdout);
        let mut term = Terminal::new(backend).map_err(RecordError::SetUpTerminal)?;

        let dump_panics = std::env::var_os("SCM_RECORD_DUMP_PANICS").is_some();
        let recorder = Self {
            state,
            use_unicode: true,
            selection: SelectionKey::None,
        };
        let state = if dump_panics {
            recorder.run_inner(&mut term)
        } else {
            // Catch any panics and restore terminal state, since otherwise the
            // terminal will be mostly unusable.
            let state = panic::catch_unwind(
                // HACK: I don't actually know if the terminal is unwind-safe 🙃.
                AssertUnwindSafe(|| recorder.run_inner(&mut term)),
            );
            if let Err(err) = Self::clean_up(&mut term) {
                warn!(?err, "Failed to clean up terminal");
            }
            match state {
                Ok(state) => state,
                Err(panic) => {
                    // HACK: it should be possible to just call
                    //
                    //     panic::resume_unwind(panic)
                    //
                    // but, for some reason, when I do that, the panic information
                    // is not printed. Then it generally looks like the program
                    // exited successfully but did nothing. This at least ensures
                    // that *something* is printed to indicate that there was a
                    // panic, even if it doesn't include all of the panic details.
                    if let Some(payload) = panic.downcast_ref::<String>() {
                        panic!("panic occurred: {payload}");
                    } else if let Some(payload) = panic.downcast_ref::<&str>() {
                        panic!("panic occurred: {payload}");
                    } else {
                        panic!("panic occurred (message not available)");
                    }
                }
            }
        };
        state
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

    fn run_inner(mut self, term: &mut CrosstermTerminal) -> Result<RecordState<'a>, RecordError> {
        self.selection = self.first_selection_key();

        let mut scroll_offset_y = 0;
        loop {
            let layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(100)]);

            let file_views: Vec<FileView> = self
                .state
                .files
                .iter()
                .enumerate()
                .map(|(file_idx, file)| {
                    let file_key = FileKey { file_idx };
                    let file_tristate = self.file_tristate(file_key).unwrap();
                    FileView {
                        tristate_box: TristateBox {
                            use_unicode: self.use_unicode,
                            tristate: file_tristate,
                        },
                        path: &file.path,
                        section_views: {
                            let mut section_views = Vec::new();
                            let total_num_sections = file
                                .sections
                                .iter()
                                .filter(|section| section.is_editable())
                                .count();

                            let mut section_num = 0;
                            for (section_idx, section) in file.sections.iter().enumerate() {
                                let section_key = SectionKey {
                                    file_idx,
                                    section_idx,
                                };
                                let section_tristate = self.section_tristate(section_key).unwrap();
                                if section.is_editable() {
                                    section_num += 1;
                                }
                                section_views.push(SectionView {
                                    use_unicode: self.use_unicode,
                                    tristate_box: TristateBox {
                                        use_unicode: self.use_unicode,
                                        tristate: section_tristate,
                                    },
                                    section_num,
                                    total_num_sections,
                                    section,
                                });
                            }
                            section_views
                        },
                    }
                })
                .collect();
            let app = App { file_views };

            // Ensure that at least one line is always visible.
            let term_height = usize::from(term.get_frame().size().height);
            let max_scroll_offset_y = app.height().saturating_sub(1);

            scroll_offset_y = scroll_offset_y.clamp(0, max_scroll_offset_y);

            term.draw(|frame| {
                let chunks = layout.split(frame.size());
                let widget = TopLevelComponentWidget {
                    app,
                    viewport_x: 0,
                    viewport_y: scroll_offset_y.unwrap_isize(),
                };
                frame.render_widget(widget, chunks[0]);
            })
            .map_err(RecordError::RenderFrame)?;

            match crossterm::event::read().map_err(RecordError::ReadInput)? {
                Event::Key(
                    KeyEvent {
                        code: KeyCode::Char('q'),
                        modifiers: KeyModifiers::NONE,
                        kind: KeyEventKind::Press,
                        state: _,
                    }
                    | KeyEvent {
                        code: KeyCode::Char('c'),
                        modifiers: KeyModifiers::CONTROL,
                        kind: KeyEventKind::Press,
                        state: _,
                    },
                ) => break,

                Event::Key(KeyEvent {
                    code: KeyCode::Up,
                    modifiers: KeyModifiers::NONE,
                    kind: KeyEventKind::Press,
                    state: _,
                })
                | Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    column: _,
                    row: _,
                    modifiers: _,
                }) => {
                    scroll_offset_y = scroll_offset_y.saturating_sub(1);
                }
                Event::Key(KeyEvent {
                    code: KeyCode::Down,
                    modifiers: KeyModifiers::NONE,
                    kind: KeyEventKind::Press,
                    state: _,
                })
                | Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollDown,
                    column: _,
                    row: _,
                    modifiers: _,
                }) => {
                    scroll_offset_y = scroll_offset_y.saturating_add(1);
                }

                Event::Key(
                    KeyEvent {
                        code: KeyCode::PageUp,
                        modifiers: KeyModifiers::NONE,
                        kind: KeyEventKind::Press,
                        state: _,
                    }
                    | KeyEvent {
                        code: KeyCode::Char('u'),
                        modifiers: KeyModifiers::CONTROL,
                        kind: KeyEventKind::Press,
                        state: _,
                    },
                ) => {
                    scroll_offset_y = scroll_offset_y.saturating_sub(term_height);
                }
                Event::Key(
                    KeyEvent {
                        code: KeyCode::PageDown,
                        modifiers: KeyModifiers::NONE,
                        kind: KeyEventKind::Press,
                        state: _,
                    }
                    | KeyEvent {
                        code: KeyCode::Char('d'),
                        modifiers: KeyModifiers::CONTROL,
                        kind: KeyEventKind::Press,
                        state: _,
                    },
                ) => {
                    scroll_offset_y = scroll_offset_y.saturating_add(term_height);
                }

                Event::Key(KeyEvent {
                    code: KeyCode::Char(' '),
                    modifiers: KeyModifiers::NONE,
                    kind: KeyEventKind::Press,
                    state: _,
                }) => {
                    self.toggle_current_item()?;
                }

                _event => {}
            }
        }

        Ok(self.state)
    }

    fn toggle_current_item(&mut self) -> Result<(), RecordError> {
        match self.selection {
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
                            } => {
                                *is_toggled = is_toggled_new;
                            }
                        }
                    }
                })?;
            }
            SelectionKey::Section(section_key) => {
                let tristate = self.section_tristate(section_key)?;
                let is_selected_new = match tristate {
                    Tristate::Unchecked => true,
                    Tristate::Partial | Tristate::Checked => false,
                };
                self.visit_section(section_key, |section| match section {
                    Section::Unchanged { .. } => {}
                    Section::Changed { lines } => {
                        for line in lines {
                            line.is_toggled = is_selected_new;
                        }
                    }
                    Section::FileMode {
                        is_toggled,
                        before: _,
                        after: _,
                    } => {
                        *is_toggled = is_selected_new;
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

    fn first_selection_key(&self) -> SelectionKey {
        match self.state.files.iter().enumerate().next() {
            Some((file_idx, _)) => SelectionKey::File(FileKey { file_idx }),
            None => SelectionKey::None,
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
                            (None, is_selected) => Some(is_selected),
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
                } => {
                    seen_value = match (seen_value, is_toggled) {
                        (None, is_selected) => Some(*is_selected),
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
                        (None, is_selected) => Some(is_selected),
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
            section @ (Section::Unchanged { lines: _ }
            | Section::FileMode {
                is_toggled: _,
                before: _,
                after: _,
            }) => Err(RecordError::Bug(format!(
                "Bad line key {line_key:?}, tried to index section {section:?}"
            ))),
        }
    }
}

#[derive(Debug)]
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

#[derive(Debug)]
struct TristateBox {
    pub(crate) use_unicode: bool,
    pub(crate) tristate: Tristate,
}

impl TristateBox {
    fn text(&self) -> &'static str {
        let Self {
            use_unicode,
            tristate,
        } = self;
        match tristate {
            Tristate::Unchecked => "[ ] ",
            Tristate::Partial => "[~] ",
            Tristate::Checked => {
                if *use_unicode {
                    "[✕] "
                } else {
                    "[x] "
                }
            }
        }
    }

    pub fn width(&self) -> usize {
        self.text().chars().count()
    }
}

impl Component for TristateBox {
    fn draw(&self, viewport: &mut Viewport, x: isize, y: isize) {
        let span = Span::styled(self.text(), Style::default().add_modifier(Modifier::BOLD));
        viewport.draw_span(x, y, &span);
    }
}

#[derive(Debug)]
struct App<'a> {
    file_views: Vec<FileView<'a>>,
}

impl App<'_> {
    fn height(&self) -> usize {
        let Self { file_views } = self;
        file_views.iter().map(|file_view| file_view.height()).sum()
    }
}

impl Component for App<'_> {
    fn draw(&self, viewport: &mut Viewport, x: isize, y: isize) {
        let Self { file_views } = self;

        let mut y = y;
        for file_view in file_views {
            file_view.draw(viewport, x, y);
            y += file_view.height().unwrap_isize();
        }
    }
}

#[derive(Debug)]
struct FileView<'a> {
    pub(crate) tristate_box: TristateBox,
    pub(crate) path: &'a Path,
    pub(crate) section_views: Vec<SectionView<'a>>,
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
    fn draw(&self, viewport: &mut Viewport, x: isize, y: isize) {
        let Self {
            tristate_box,
            path,
            section_views: sections,
        } = self;

        tristate_box.draw(viewport, x, y);
        viewport.draw_span(
            x + tristate_box.width().unwrap_isize(),
            y,
            &Span::styled(path.to_string_lossy(), Style::default()),
        );

        let x = x + 2;
        let mut y = y + 1;
        for section_view in sections {
            let section_area = Rect {
                x,
                y,
                width: viewport.size().width,
                height: section_view.height(),
            };
            if viewport.contains(section_area) {
                section_view.draw(viewport, section_area.x, section_area.y);
            }
            y += section_area.height.unwrap_isize();
        }
    }
}

#[derive(Debug)]
struct SectionView<'a> {
    use_unicode: bool,
    tristate_box: TristateBox,
    section_num: usize,
    total_num_sections: usize,
    section: &'a Section<'a>,
}

impl SectionView<'_> {
    pub fn height(&self) -> usize {
        let header_height = if self.section.is_editable() { 1 } else { 0 };
        header_height
            + match self.section {
                Section::Unchanged { lines } => lines.len(),
                Section::Changed { lines } => lines.len(),
                Section::FileMode { .. } => 1,
            }
    }
}

impl Component for SectionView<'_> {
    fn draw(&self, viewport: &mut Viewport, x: isize, y: isize) {
        let Self {
            use_unicode,
            tristate_box,
            section_num,
            total_num_sections,
            section,
        } = self;

        let y = if !section.is_editable() {
            y
        } else {
            tristate_box.draw(viewport, x, y);
            viewport.draw_span(
                x + tristate_box.width().unwrap_isize(),
                y,
                &Span::styled(
                    format!("Section {section_num}/{total_num_sections}"),
                    Style::default(),
                ),
            );
            y + 1
        };
        let x = x + 2;

        match section {
            Section::Unchanged { lines } => {
                // TODO: only display a certain number of contextual lines
                let x = x + tristate_box.width().unwrap_isize() + "+ ".len().unwrap_isize();
                for (dy, line) in lines.iter().enumerate() {
                    let span = Span::styled(line.as_ref(), Style::default());
                    viewport.draw_span(x, y + dy.unwrap_isize(), &span);
                }
            }

            Section::Changed { lines } => {
                for (dy, line) in lines.iter().enumerate() {
                    let y = y + dy.unwrap_isize();
                    let SectionChangedLine {
                        is_toggled: is_selected,
                        change_type,
                        line,
                    } = line;

                    let tristate_box = TristateBox {
                        use_unicode: *use_unicode,
                        tristate: Tristate::from(*is_selected),
                    };
                    tristate_box.draw(viewport, x, y);
                    let x = x + tristate_box.width().unwrap_isize();

                    let (change_type_text, style) = match change_type {
                        ChangeType::Added => ("+ ", Style::default().fg(Color::Green)),
                        ChangeType::Removed => ("- ", Style::default().fg(Color::Red)),
                    };
                    viewport.draw_span(x, y, &Span::styled(change_type_text, style));
                    let x = x + change_type_text.chars().count().unwrap_isize();
                    viewport.draw_span(x, y, &Span::styled(line.as_ref(), style));
                }
            }

            Section::FileMode { .. } => unimplemented!("rendering file mode section"),
        }
    }
}
