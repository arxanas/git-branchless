use std::fmt::Write;
use std::io::{stderr, stdout, Write as WriteIo};
use std::sync::{Arc, Mutex};

use crate::core::formatting::Glyphs;

#[derive(Clone)]
enum OutputDest {
    Stdout,
    BufferForTest(Arc<Mutex<Vec<u8>>>),
}

/// Wrapper around output. Also manages progress indicators.
#[derive(Clone)]
pub struct Output {
    glyphs: Arc<Glyphs>,
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
            glyphs: Arc::new(glyphs),
            dest: OutputDest::Stdout,
        }
    }

    /// Constructor. Writes to the provided buffer.
    pub fn new_from_buffer_for_test(glyphs: Glyphs, buffer: &Arc<Mutex<Vec<u8>>>) -> Self {
        Output {
            glyphs: Arc::new(glyphs),
            dest: OutputDest::BufferForTest(Arc::clone(buffer)),
        }
    }

    /// Get the set of glyphs associated with the output.
    pub fn get_glyphs(&self) -> Arc<Glyphs> {
        Arc::clone(&self.glyphs)
    }

    /// Create a stream that error output can be written to, rather than regular
    /// output.
    pub fn into_error_stream(self) -> ErrorOutput {
        ErrorOutput { output: self }
    }
}

impl Write for Output {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        match &self.dest {
            OutputDest::Stdout => {
                print!("{}", s);
                stdout().flush().unwrap();
            }
            OutputDest::BufferForTest(buffer) => {
                let mut buffer = buffer.lock().unwrap();
                write!(buffer, "{}", s).unwrap();
            }
        }
        Ok(())
    }
}

pub struct ErrorOutput {
    output: Output,
}

impl Write for ErrorOutput {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        match &self.output.dest {
            OutputDest::Stdout => {
                eprint!("{}", s);
                stderr().flush().unwrap();
            }
            OutputDest::BufferForTest(_) => {
                // Drop the error output, as the buffer only represents `stdout`.
            }
        }
        Ok(())
    }
}
