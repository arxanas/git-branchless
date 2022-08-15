use cursive::{
    direction::Direction,
    event::{Event, EventResult, Key, MouseButton, MouseEvent},
    impl_enabled,
    theme::ColorStyle,
    view::{CannotFocus, View},
    Cursive, Printer, Vec2, With,
};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tristate {
    Unchecked,
    Partial,
    Checked,
}

type TristateBoxChangeFn = dyn Fn(&mut Cursive, Tristate);

/// Three-state checkable box.
pub struct TristateBox {
    state: Tristate,
    enabled: bool,

    on_change: Option<Rc<TristateBoxChangeFn>>,
}

impl TristateBox {
    impl_enabled!(self.enabled);

    /// Creates a new, unset tristate box.
    pub fn new() -> Self {
        Self {
            state: Tristate::Unchecked,
            enabled: true,
            on_change: None,
        }
    }

    /// Sets a callback to be used when the state changes.
    pub fn set_on_change<F: 'static + Fn(&mut Cursive, Tristate)>(&mut self, on_change: F) {
        self.on_change = Some(Rc::new(on_change));
    }

    /// Sets a callback to be used when the state changes.
    ///
    /// Chainable variant.
    #[must_use]
    pub fn on_change<F: 'static + Fn(&mut Cursive, Tristate)>(self, on_change: F) -> Self {
        self.with(|s| s.set_on_change(on_change))
    }

    /// Toggles the checkbox state.
    pub fn toggle(&mut self) -> EventResult {
        let state = match self.state {
            Tristate::Unchecked | Tristate::Partial => Tristate::Checked,
            Tristate::Checked => Tristate::Unchecked,
        };
        self.set_state(state)
    }

    /// Sets the checkbox state.
    pub fn set_state(&mut self, state: Tristate) -> EventResult {
        self.state = state;
        if let Some(ref on_change) = self.on_change {
            let on_change = Rc::clone(on_change);
            EventResult::with_cb(move |s| on_change(s, state))
        } else {
            EventResult::Consumed(None)
        }
    }

    /// Set the checkbox state.
    ///
    /// Chainable variant.
    #[must_use]
    pub fn with_state(self, state: Tristate) -> Self {
        self.with(|s| {
            s.set_state(state);
        })
    }

    fn draw_internal(&self, printer: &Printer) {
        printer.print(
            (0, 0),
            match self.state {
                Tristate::Unchecked => "[ ]",
                Tristate::Partial => "[~]",
                Tristate::Checked => "[X]",
            },
        );
    }
}

impl View for TristateBox {
    fn required_size(&mut self, _: Vec2) -> Vec2 {
        Vec2::new(3, 1)
    }

    fn take_focus(&mut self, _: Direction) -> Result<EventResult, CannotFocus> {
        self.enabled.then(EventResult::consumed).ok_or(CannotFocus)
    }

    fn draw(&self, printer: &Printer) {
        if self.enabled && printer.enabled {
            printer.with_selection(printer.focused, |printer| self.draw_internal(printer));
        } else {
            printer.with_color(ColorStyle::secondary(), |printer| {
                self.draw_internal(printer)
            });
        }
    }

    fn on_event(&mut self, event: Event) -> EventResult {
        if !self.enabled {
            return EventResult::Ignored;
        }
        match event {
            Event::Key(Key::Enter) | Event::Char(' ') => self.toggle(),
            Event::Mouse {
                event: MouseEvent::Release(MouseButton::Left),
                position,
                offset,
            } if position.fits_in_rect(offset, (3, 1)) => self.toggle(),
            _ => EventResult::Ignored,
        }
    }
}
