use std::{borrow::Cow, path::PathBuf};

/// The state used to render the changes. This is passed into [`Recorder::new`]
/// and then updated and returned with [`Recorder::run`].
#[derive(Clone, Debug)]
pub struct RecordState<'a> {
    /// The state of each file. This is rendered in order, so you may want to
    /// sort this list by path before providing it.
    pub file_states: Vec<(PathBuf, FileState<'a>)>,
}

/// An error which occurred when attempting to record changes.
#[derive(Clone, Debug)]
pub enum RecordError {
    /// The user cancelled the operation.
    Cancelled,
}

pub type FileMode = usize;

/// The state of a file to be recorded.
#[derive(Clone, Debug)]
pub struct FileState<'a> {
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

impl FileState<'_> {
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
            file_mode: _,
            sections,
        } = self;
        sections
            .iter()
            .filter(|section| match section {
                Section::Unchanged { .. } => false,
                Section::Changed { .. } | Section::FileMode { .. } => true,
            })
            .count()
    }

    /// Get the new Unix file mode. If the user selected a
    /// [`Section::FileMode`], then returns that file mode. Otherwise, returns
    /// the `file_mode` value that this [`FileState`] was constructed with.
    pub fn get_file_mode(&self) -> Option<FileMode> {
        let Self {
            file_mode,
            sections,
        } = self;
        sections
            .iter()
            .find_map(|section| match section {
                Section::Unchanged { .. }
                | Section::Changed { .. }
                | Section::FileMode {
                    is_selected: false,
                    before: _,
                    after: _,
                } => None,

                Section::FileMode {
                    is_selected: true,
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
            file_mode: _,
            sections,
        } = self;
        for section in sections {
            match section {
                Section::Unchanged { contents } => {
                    for line in contents {
                        acc_selected.push_str(line);
                        acc_unselected.push_str(line);
                    }
                }
                Section::Changed { before, after } => {
                    for SectionChangedLine { is_selected, line } in before {
                        // Note the inverted condition here.
                        if !*is_selected {
                            acc_selected.push_str(line);
                        } else {
                            acc_unselected.push_str(line);
                        }
                    }

                    for SectionChangedLine { is_selected, line } in after {
                        if *is_selected {
                            acc_selected.push_str(line);
                        } else {
                            acc_unselected.push_str(line);
                        }
                    }
                }
                Section::FileMode {
                    is_selected: _,
                    before: _,
                    after: _,
                } => {
                    // Do nothing; the caller should use `get_file_mode` to get this information.
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
    Unchanged {
        /// The contents of the lines in this section. Each line includes its
        /// trailing newline character(s), if any.
        contents: Vec<Cow<'a, str>>,
    },

    /// This section of the file is changed, and the user needs to select which
    /// specific changed lines to record.
    Changed {
        /// The contents of the lines before the user change was made. Each line
        /// includes its trailing newline character(s), if any.
        before: Vec<SectionChangedLine<'a>>,

        /// The contents of the lines after the user change was made. Each line
        /// includes its trailing newline character(s), if any.
        after: Vec<SectionChangedLine<'a>>,
    },

    /// The Unix file mode of the file changed, and the user needs to select
    /// whether to accept that mode change or not.
    FileMode {
        /// Whether or not the file mode change was accepted.
        is_selected: bool,

        /// The old file mode.
        before: FileMode,

        /// The new file mode.
        after: FileMode,
    },
}

/// A changed line inside a `Section`.
#[derive(Clone, Debug)]
pub struct SectionChangedLine<'a> {
    /// Whether or not this line was selected to be recorded.
    pub is_selected: bool,

    /// The contents of the line, including its trailing newline character(s),
    /// if any.
    pub line: Cow<'a, str>,
}

impl<'a> SectionChangedLine<'a> {
    /// Make a copy of this [`SectionChangedLine`] that borrows the content of
    /// the line from the original.
    pub fn borrow_line(&'a self) -> Self {
        let Self { is_selected, line } = self;
        Self {
            is_selected: *is_selected,
            line: Cow::Borrowed(line),
        }
    }
}
