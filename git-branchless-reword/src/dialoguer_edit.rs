//! Fork of `dialoguer::edit`.
//!
//! Originally from https://github.com/mitsuhiko/dialoguer/blob/40c7c90f04c8bcab4e26133fdf6ece30fd001bd0/src/edit.rs
//!
//! There are bugs we want to fix and behaviors we want to customize, and their
//! release schedule may not align with ours.  This chunk of code is fairly
//! small, so we can vendor it here.
//!
//! `dialoguer` is originally released under the MIT license:
//!
//! The MIT License (MIT)
//! Copyright (c) 2017 Armin Ronacher <armin.ronacher@active-4.com>
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
use std::process;

/// Launches the default editor to edit a string.
///
/// ## Example
///
/// ```rust,no_run
/// use git_branchless_reword::dialoguer_edit::Editor;
///
/// if let Some(rv) = Editor::new().edit("Enter a commit message").unwrap() {
///     println!("Your message:");
///     println!("{}", rv);
/// } else {
///     println!("Abort!");
/// }
/// ```
pub struct Editor {
    editor: OsString,
    extension: String,
    require_save: bool,
    trim_newlines: bool,
}

fn get_default_editor() -> OsString {
    if let Some(prog) = env::var_os("VISUAL") {
        return prog;
    }
    if let Some(prog) = env::var_os("EDITOR") {
        return prog;
    }
    if cfg!(windows) {
        "notepad.exe".into()
    } else {
        "vi".into()
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    /// Creates a new editor.
    pub fn new() -> Self {
        Self {
            editor: get_default_editor(),
            extension: ".txt".into(),
            require_save: true,
            trim_newlines: true,
        }
    }

    /// Sets a specific editor executable.
    pub fn executable<S: AsRef<OsStr>>(&mut self, val: S) -> &mut Self {
        self.editor = val.as_ref().into();
        self
    }

    /// Sets a specific extension
    pub fn extension(&mut self, val: &str) -> &mut Self {
        self.extension = val.into();
        self
    }

    /// Enables or disables the save requirement.
    pub fn require_save(&mut self, val: bool) -> &mut Self {
        self.require_save = val;
        self
    }

    /// Enables or disables trailing newline stripping.
    ///
    /// This is on by default.
    pub fn trim_newlines(&mut self, val: bool) -> &mut Self {
        self.trim_newlines = val;
        self
    }

    /// Launches the editor to edit a string.
    ///
    /// Returns `None` if the file was not saved or otherwise the
    /// entered text.
    pub fn edit(&self, s: &str) -> io::Result<Option<String>> {
        let mut f = tempfile::Builder::new()
            .prefix("edit-")
            .suffix(&self.extension)
            .rand_bytes(12)
            .tempfile()?;
        f.write_all(s.as_bytes())?;
        f.flush()?;
        let ts = fs::metadata(f.path())?.modified()?;

        let s: String = self.editor.clone().into_string().unwrap();
        let (cmd, args) = match shell_words::split(&s) {
            Ok(mut parts) => {
                let cmd = parts.remove(0);
                (cmd, parts)
            }
            Err(_) => (s, vec![]),
        };

        let rv = process::Command::new(cmd)
            .args(args)
            .arg(f.path())
            .spawn()?
            .wait()?;

        if rv.success() && self.require_save && ts >= fs::metadata(f.path())?.modified()? {
            return Ok(None);
        }

        let mut new_f = fs::File::open(f.path())?;
        let mut rv = String::new();
        new_f.read_to_string(&mut rv)?;

        if self.trim_newlines {
            let len = rv.trim_end_matches(&['\n', '\r'][..]).len();
            rv.truncate(len);
        }

        Ok(Some(rv))
    }
}
