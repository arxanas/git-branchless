//! TODO: extract into own crate

use std::panic::{self, AssertUnwindSafe, RefUnwindSafe, UnwindSafe};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

use cursive::event::Event;
use cursive::{CursiveRunnable, CursiveRunner};

pub trait EventDrivenCursiveApp
where
    Self: UnwindSafe,
{
    type Message: Clone + std::fmt::Debug + UnwindSafe + 'static;
    type Return;

    fn get_init_message(&self) -> Self::Message;
    fn get_key_bindings(&self) -> Vec<(Event, Self::Message)>;
    fn handle_message(
        &mut self,
        siv: &mut CursiveRunner<CursiveRunnable>,
        main_tx: Sender<Self::Message>,
        message: Self::Message,
    );
    fn finish(self) -> Self::Return;
}

pub trait EventDrivenCursiveAppExt: EventDrivenCursiveApp {
    fn run(self, siv: CursiveRunner<CursiveRunnable>) -> Self::Return;
}

impl<T: EventDrivenCursiveApp + UnwindSafe + RefUnwindSafe> EventDrivenCursiveAppExt for T {
    fn run(mut self, mut siv: CursiveRunner<CursiveRunnable>) -> T::Return {
        let (main_tx, main_rx): (Sender<T::Message>, Receiver<T::Message>) = channel();

        self.get_key_bindings().iter().cloned().for_each(
            |(event, message): (cursive::event::Event, T::Message)| {
                siv.add_global_callback(event, {
                    let main_tx = main_tx.clone();
                    move |_siv| main_tx.send(message.clone()).unwrap()
                });
            },
        );

        main_tx.send(self.get_init_message()).unwrap();
        while siv.is_running() {
            let message = main_rx.try_recv();
            if message.is_err() {
                // For tests: only pump the Cursive event loop if we have no events
                // of our own to process. Otherwise, the event loop queues up all of
                // the messages before we can process them, which means that none of
                // the screenshots are correct.
                siv.step();
            }

            match message {
                Err(TryRecvError::Disconnected) => break,

                Err(TryRecvError::Empty) => {
                    // If we haven't received a message yet, defer to `siv.step`
                    // to process the next user input.
                    continue;
                }

                Ok(message) => {
                    let maybe_panic = panic::catch_unwind({
                        let mut siv = AssertUnwindSafe(&mut siv);
                        let mut self_ = AssertUnwindSafe(&mut self);
                        let main_tx = AssertUnwindSafe(main_tx.clone());
                        move || {
                            self_.handle_message(&mut siv, main_tx.clone(), message);
                        }
                    });
                    match maybe_panic {
                        Ok(()) => {
                            siv.refresh();
                        }
                        Err(panic) => {
                            // Ensure we exit TUI mode before attempting to print panic details.
                            drop(siv);
                            if let Some(payload) = panic.downcast_ref::<String>() {
                                panic!("panic occurred: {payload}");
                            } else if let Some(payload) = panic.downcast_ref::<&str>() {
                                panic!("panic occurred: {payload}");
                            } else {
                                panic!("panic occurred (message not available)",);
                            }
                        }
                    }
                }
            };
        }

        self.finish()
    }
}

/// Testing helpers for interactive interfaces.
pub mod testing {
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
    }

    impl CursiveTestingBackend {
        /// Construct the testing backend with the provided set of events.
        pub fn init(events: Vec<CursiveTestingEvent>) -> Box<dyn Backend> {
            Box::new(CursiveTestingBackend {
                events,
                event_index: 0,
                just_emitted_event: false,
                screen: RefCell::new(vec![vec![' '; 120]; 24]),
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
                    *screen_target = self.screen.borrow().clone();
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

        fn print_at(&self, pos: cursive::Vec2, text: &str) {
            for (i, c) in text.chars().enumerate() {
                let mut screen = self.screen.borrow_mut();
                let screen_width = screen[0].len();
                if pos.x + i < screen_width {
                    screen[pos.y][pos.x + i] = c;
                } else {
                    // Indicate that the screen was overfull.
                    screen[pos.y][screen_width - 1] = '$';
                }
            }
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
}
