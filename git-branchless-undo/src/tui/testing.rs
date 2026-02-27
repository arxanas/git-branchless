//! Testing helpers for interactive interfaces.

use std::borrow::Borrow;
use std::cell::RefCell;
use std::rc::Rc;

use cursive::backend::Backend;
use cursive::theme::Color;

/// Represents a "screenshot" of the terminal taken at a point in time.
pub type Screen = Vec<Vec<char>>;

/// The kind of events that can be
#[derive(Clone, Debug)]
pub enum CursiveTestingEvent {
    /// A regular Cursive event.
    Event(cursive::event::Event),

    /// Take a screenshot at the current point in time and store it in the
    /// provided screenshot cell.
    TakeScreenshot(Rc<RefCell<Screen>>),
}

/// The testing backend. It feeds a predetermined list of events to the
/// Cursive event loop and stores a virtual terminal for Cursive to draw on.
#[derive(Debug)]
pub struct CursiveTestingBackend {
    events: Vec<CursiveTestingEvent>,
    event_index: usize,
    just_emitted_event: bool,
    screen: RefCell<Screen>,
    cursor_pos: RefCell<cursive::Vec2>,
}

impl CursiveTestingBackend {
    /// Construct the testing backend with the provided set of events.
    pub fn init(events: Vec<CursiveTestingEvent>) -> Box<dyn Backend> {
        Box::new(CursiveTestingBackend {
            events,
            event_index: 0,
            just_emitted_event: false,
            screen: RefCell::new(vec![vec![' '; 120]; 24]),
            cursor_pos: RefCell::new(cursive::Vec2::zero()),
        })
    }
}

impl Backend for CursiveTestingBackend {
    fn poll_event(&mut self) -> Option<cursive::event::Event> {
        // Cursive will poll all available events. We only want it to
        // process events one at a time, so return `None` after each event.
        if self.just_emitted_event {
            self.just_emitted_event = false;
            return None;
        }

        let event_index = self.event_index;
        self.event_index += 1;
        match self.events.get(event_index)?.to_owned() {
            CursiveTestingEvent::TakeScreenshot(screen_target) => {
                let mut screen_target = (*screen_target).borrow_mut();
                screen_target.clone_from(&self.screen.borrow());
                self.poll_event()
            }
            CursiveTestingEvent::Event(event) => {
                self.just_emitted_event = true;
                Some(event)
            }
        }
    }

    fn refresh(&mut self) {}

    fn has_colors(&self) -> bool {
        false
    }

    fn screen_size(&self) -> cursive::Vec2 {
        let screen = self.screen.borrow();
        (screen[0].len(), screen.len()).into()
    }

    fn move_to(&self, pos: cursive::Vec2) {
        *self.cursor_pos.borrow_mut() = pos;
    }

    fn print(&self, text: &str) {
        let pos = *self.cursor_pos.borrow();
        let mut col = pos.x;
        for c in text.chars() {
            let mut screen = self.screen.borrow_mut();
            let screen_width = screen[0].len();
            if col < screen_width {
                screen[pos.y][col] = c;
                col += 1;
            } else {
                // Indicate that the screen was overfull.
                screen[pos.y][screen_width - 1] = '$';
                break;
            }
        }
        self.cursor_pos.borrow_mut().x = col;
    }

    fn clear(&self, _color: Color) {
        let mut screen = self.screen.borrow_mut();
        for i in 0..screen.len() {
            for j in 0..screen[i].len() {
                screen[i][j] = ' ';
            }
        }
    }

    fn set_color(&self, colors: cursive::theme::ColorPair) -> cursive::theme::ColorPair {
        colors
    }

    fn set_effect(&self, _effect: cursive::theme::Effect) {}

    fn unset_effect(&self, _effect: cursive::theme::Effect) {}

    fn set_title(&mut self, _title: String) {}
}

/// Convert the screenshot into a string for assertions, such as for use
/// with `insta::assert_snapshot!`.
pub fn screen_to_string(screen: &Rc<RefCell<Screen>>) -> String {
    let screen = Rc::borrow(screen);
    let screen = RefCell::borrow(screen);
    screen
        .iter()
        .map(|row| {
            let line: String = row.iter().collect();
            line.trim_end().to_owned() + "\n"
        })
        .collect::<String>()
        .trim()
        .to_owned()
}
