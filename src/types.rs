//! Data types for the change selector interface.

use std::borrow::Cow;
use std::io;
use std::path::Path;

use thiserror::Error;

/// The state used to render the changes. This is passed into [`Recorder::new`]
/// and then updated and returned with [`Recorder::run`].
#[derive(Clone, Debug, Default)]
pub struct RecordState<'a> {
    /// The state of each file. This is rendered in order, so you may want to
    /// sort this list by path before providing it.
    pub files: Vec<File<'a>>,
}

/// An error which occurred when attempting to record changes.
#[allow(missing_docs)]
#[derive(Debug, Error)]
pub enum RecordError {
    /// The user cancelled the operation.
    #[error("cancelled by user")]
    Cancelled,

    #[error("failed to set up terminal: {0}")]
    SetUpTerminal(#[source] io::Error),

    #[error("failed to render new frame: {0}")]
    RenderFrame(#[source] io::Error),

    #[error("failed to read user input: {0}")]
    ReadInput(#[source] crossterm::ErrorKind),

    #[error("bug: {0}")]
    Bug(String),
}

/// The Unix file mode.
pub type FileMode = usize;

/// The state of a file to be recorded.
#[derive(Clone, Debug)]
pub struct File<'a> {
    /// The path to the file.
    pub path: Cow<'a, Path>,

    /// The Unix file mode of the file, if available.
    ///
    /// This value is not directly modified by the UI; instead, construct a
    /// [`Section::FileMode`] and use the [`FileState::get_file_mode`] function
    /// to read a user-provided updated to the file mode function to read a
    /// user-provided updated to the file mode
    pub file_mode: Option<FileMode>,

    /// The set of [`Section`]s inside the file.
    pub sections: Vec<Section<'a>>,
}

impl File<'_> {
    /// An absent file.
    pub fn absent() -> Self {
        unimplemented!("FileState::absent")
    }

    /// A binary file.
    pub fn binary() -> Self {
        unimplemented!("FileState::binary")
    }

    /// Count the number of changed sections in this file.
    pub fn count_changed_sections(&self) -> usize {
        let Self {
            path: _,
            file_mode: _,
            sections,
        } = self;
        sections
            .iter()
            .filter(|section| match section {
                Section::Unchanged { .. } => false,
                Section::Changed { .. } => true,
                Section::FileMode { .. } => {
                    unimplemented!("count_changed_sections for Section::FileMode")
                }
            })
            .count()
    }

    /// Get the new Unix file mode. If the user selected a
    /// [`Section::FileMode`], then returns that file mode. Otherwise, returns
    /// the `file_mode` value that this [`FileState`] was constructed with.
    pub fn get_file_mode(&self) -> Option<FileMode> {
        let Self {
            path: _,
            file_mode,
            sections,
        } = self;
        sections
            .iter()
            .find_map(|section| match section {
                Section::Unchanged { .. }
                | Section::Changed { .. }
                | Section::FileMode {
                    is_toggled: false,
                    before: _,
                    after: _,
                } => None,

                Section::FileMode {
                    is_toggled: true,
                    before: _,
                    after,
                } => Some(*after),
            })
            .or(*file_mode)
    }

    /// Calculate the `(selected, unselected)` contents of the file. For
    /// example, the first value would be suitable for staging or committing,
    /// and the second value would be suitable for potentially recording again.
    pub fn get_selected_contents(&self) -> (String, String) {
        let mut acc_selected = String::new();
        let mut acc_unselected = String::new();
        let Self {
            path: _,
            file_mode: _,
            sections,
        } = self;
        for section in sections {
            match section {
                Section::Unchanged { lines } => {
                    for line in lines {
                        acc_selected.push_str(line);
                        acc_selected.push('\n');
                        acc_unselected.push_str(line);
                        acc_unselected.push('\n');
                    }
                }
                Section::Changed { lines } => {
                    for line in lines {
                        let SectionChangedLine {
                            is_toggled: is_selected,
                            change_type,
                            line,
                        } = line;
                        match (change_type, is_selected) {
                            (ChangeType::Added, true) | (ChangeType::Removed, false) => {
                                acc_selected.push_str(line);
                                acc_selected.push('\n');
                            }
                            (ChangeType::Added, false) | (ChangeType::Removed, true) => {
                                acc_unselected.push_str(line);
                                acc_unselected.push('\n');
                            }
                        }
                    }
                }
                Section::FileMode {
                    is_toggled: _,
                    before: _,
                    after: _,
                } => {
                    unimplemented!("get_selected_contents for Section::FileMode");
                }
            }
        }
        (acc_selected, acc_unselected)
    }
}

/// A section of a file to be rendered and recorded.
#[derive(Clone, Debug)]
pub enum Section<'a> {
    /// This section of the file is unchanged and just used for context.
    ///
    /// By default, only part of the context will be shown. However, all of the
    /// context lines should be provided so that they can be used to globally
    /// number the lines correctly.
    Unchanged {
        /// The contents of the lines in this section. Each line does *not*
        /// include a trailing newline character.
        lines: Vec<Cow<'a, str>>,
    },

    /// This section of the file is changed, and the user needs to select which
    /// specific changed lines to record.
    Changed {
        /// The contents of the lines caused by the user change. Each line does
        /// *not* include a trailing newline character.
        lines: Vec<SectionChangedLine<'a>>,
    },

    /// This indicates that the Unix file mode of the file changed, and that the
    /// user needs to accept that mode change or not. This is not part of the
    /// "contents" of the file per se, but it's rendered inline as if it were.
    FileMode {
        /// Whether or not the file mode change was accepted.
        is_toggled: bool,

        /// The old file mode.
        before: FileMode,

        /// The new file mode.
        after: FileMode,
    },
}

impl Section<'_> {
    /// Whether or not this section contains user-editable content (as opposed
    /// to simply contextual content).
    pub fn is_editable(&self) -> bool {
        match self {
            Section::Unchanged { .. } => false,
            Section::Changed { .. } | Section::FileMode { .. } => true,
        }
    }
}

/// The type of change in the patch/diff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeType {
    /// The line was added.
    Added,

    /// The line was removed.
    Removed,
}

/// A changed line inside a `Section`.
#[derive(Clone, Debug)]
pub struct SectionChangedLine<'a> {
    /// Whether or not this line was selected to be recorded.
    pub is_toggled: bool,

    /// The type of change this line was.
    pub change_type: ChangeType,

    /// The contents of the line, including its trailing newline character(s),
    /// if any.
    pub line: Cow<'a, str>,
}
