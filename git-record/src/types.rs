use std::path::PathBuf;

/// The state used to render the changes. This is passed into [`Recorder::new`]
/// and then updated and returned with [`Recorder::run`].
#[derive(Clone, Debug)]
pub struct RecordState {
    /// The state of each file. This is rendered in order, so you may want to
    /// sort this list by path before providing it.
    pub files: Vec<(PathBuf, FileContent)>,
}

/// An error which occurred when attempting to record changes.
#[derive(Clone, Debug)]
pub enum RecordError {
    /// The user cancelled the operation.
    Cancelled,
}

/// The state of a file to be recorded.
#[derive(Clone, Debug)]
pub enum FileContent {
    /// The file didn't exist. (Perhaps it hasn't yet been created, or it was deleted.)
    Absent,

    /// The file contained undisplayable binary content.
    Binary,

    /// The file contains textual content (a sequence of lines each ending with the newline character.)
    Text {
        /// The file modes before and after the change.
        file_mode: (usize, usize),

        /// The set of [`Hunk`]s inside the file.
        hunks: Vec<Hunk>,
    },
}

impl FileContent {
    /// Count the number of changed sections in this file.
    pub fn count_changed_hunks(&self) -> usize {
        match self {
            FileContent::Absent | FileContent::Binary => unimplemented!(),
            FileContent::Text {
                file_mode: _,
                hunks,
            } => hunks
                .iter()
                .filter(|hunk| match hunk {
                    Hunk::Unchanged { .. } => false,
                    Hunk::Changed { .. } => true,
                })
                .count(),
        }
    }

    /// Calculate the `(selected, unselected)` contents of the file. For
    /// example, the first value would be suitable for staging or committing,
    /// and the second value would be suitable for potentially recording again.
    pub fn get_selected_contents(&self) -> (String, String) {
        let mut acc_selected = String::new();
        let mut acc_unselected = String::new();
        match self {
            FileContent::Absent | FileContent::Binary => unimplemented!(),
            FileContent::Text {
                file_mode: _,
                hunks,
            } => {
                for hunk in hunks {
                    match hunk {
                        Hunk::Unchanged { contents } => {
                            for line in contents {
                                acc_selected.push_str(line);
                                acc_unselected.push_str(line);
                            }
                        }
                        Hunk::Changed { before, after } => {
                            for HunkChangedLine { is_selected, line } in before {
                                // Note the inverted condition here.
                                if !*is_selected {
                                    acc_selected.push_str(line);
                                } else {
                                    acc_unselected.push_str(line);
                                }
                            }

                            for HunkChangedLine { is_selected, line } in after {
                                if *is_selected {
                                    acc_selected.push_str(line);
                                } else {
                                    acc_unselected.push_str(line);
                                }
                            }
                        }
                    }
                }
            }
        }
        (acc_selected, acc_unselected)
    }
}

/// A section of a file to be rendered and recorded.
///
/// Unlike typical `diff` terminology, here, a "hunk" can refer to either a
/// changed or unchanged section of a file, not just a changed section.
#[derive(Clone, Debug)]
pub enum Hunk {
    /// This section of the file is unchanged and just used for context.
    Unchanged {
        /// The contents of the lines in this section. Each line includes its
        /// trailing newline character(s), if any.
        contents: Vec<String>,
    },

    /// This section of the file is changed, and the user needs to select which
    /// specific changed lines to record.
    Changed {
        /// The contents of the lines before the user change was made. Each line
        /// includes its trailing newline character(s), if any.
        before: Vec<HunkChangedLine>,

        /// The contents of the lines after the user change was made. Each line
        /// includes its trailing newline character(s), if any.
        after: Vec<HunkChangedLine>,
    },
}

/// A changed line inside a `Hunk`.
#[derive(Clone, Debug)]
pub struct HunkChangedLine {
    /// Whether or not this line was selected to be recorded.
    pub is_selected: bool,

    /// The contents of the line, including its trailing newline character(s),
    /// if any.
    pub line: String,
}
