use std::cell::RefCell;
use std::fmt::Write;
use std::io::{stdout, Write as WriteIo};
use std::rc::Rc;

use crate::core::formatting::Glyphs;

enum OutputDest {
    Stdout,
    Buffer(Rc<RefCell<Vec<u8>>>),
}

/// Wrapper around output. Also manages progress indicators.
pub struct Output {
    glyphs: Rc<Glyphs>,
    dest: OutputDest,
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "<Output fancy={}>",
            self.glyphs.should_write_ansi_escape_codes
        )
    }
}

impl Output {
    /// Constructor. Writes to stdout.
    pub fn new(glyphs: Glyphs) -> Self {
        Output {
            glyphs: Rc::new(glyphs),
            dest: OutputDest::Stdout,
        }
    }

    /// Constructor. Writes to the provided buffer. Panics on write if the
    /// buffer is not mutably-borrowable at runtime.
    pub fn new_from_buffer(glyphs: Glyphs, buffer: &Rc<RefCell<Vec<u8>>>) -> Self {
        Output {
            glyphs: Rc::new(glyphs),
            dest: OutputDest::Buffer(Rc::clone(buffer)),
        }
    }

    /// Get the set of glyphs associated with the output.
    pub fn get_glyphs(&self) -> Rc<Glyphs> {
        Rc::clone(&self.glyphs)
    }
}

impl Write for Output {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        match &self.dest {
            OutputDest::Stdout => {
                print!("{}", s);
                stdout().flush().unwrap();
            }
            OutputDest::Buffer(buffer) => {
                write!(buffer.borrow_mut(), "{}", s).unwrap();
            }
        }
        Ok(())
    }
}
