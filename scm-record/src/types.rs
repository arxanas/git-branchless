//! Data types for the change selector interface.

use std::borrow::Cow;
use std::fmt::Display;
use std::io;
use std::num::TryFromIntError;
use std::path::Path;

use thiserror::Error;

/// The state used to render the changes. This is passed into [`Recorder::new`]
/// and then updated and returned with [`Recorder::run`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct RecordState<'a> {
    /// Render the UI as read-only, such that the checkbox states cannot be
    /// changed by the user.
    pub is_read_only: bool,

    /// The commits containing the selected changes. Each changed section be
    /// assigned to exactly one commit.
    ///
    /// If there are fewer than two commits in this list, then it is padded to
    /// two commits using `Commit::default` before being returned.
    ///
    /// It's important to note that the `Commit`s do not literally contain the
    /// selected changes. They are stored out-of-band in the `files` field. It
    /// would be possible to store the changes in the `Commit`s, but we would no
    /// longer get the invariant that each change belongs to a single commit for
    /// free. (That being said, we now have to uphold the invariant that the
    /// changes are all assigned to valid commits.) It would also be somewhat
    /// more tedious to write the code that removes the change from one `Commit`
    /// and adds it to the correct relative position (with respect to all of the
    /// other changes) in another `Commit`.
    pub commits: Vec<Commit>,

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

    #[error("failed to clean up terminal: {0}")]
    CleanUpTerminal(#[source] io::Error),

    #[error("failed to render new frame: {0}")]
    RenderFrame(#[source] io::Error),

    #[error("failed to read user input: {0}")]
    ReadInput(#[source] io::Error),

    #[cfg(feature = "serde")]
    #[error("failed to serialize JSON: {0}")]
    SerializeJson(#[source] serde_json::Error),

    #[error("failed to wrote file: {0}")]
    WriteFile(#[source] io::Error),

    #[error("{0}")]
    Other(String),

    #[error("bug: {0}")]
    Bug(String),
}

/// The Unix file mode. The special mode `0` indicates that the file did not exist.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct FileMode(pub usize);

impl FileMode {
    /// Get the file mode corresponding to an absent file. This typically
    /// indicates that the file was created (if the "before" mode) or deleted
    /// (if the "after" mode).
    pub fn absent() -> Self {
        Self(0)
    }
}

impl Display for FileMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(mode) = self;
        write!(f, "{mode:o}")
    }
}

impl From<usize> for FileMode {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl From<FileMode> for usize {
    fn from(value: FileMode) -> Self {
        let FileMode(value) = value;
        value
    }
}

impl TryFrom<u32> for FileMode {
    type Error = TryFromIntError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Ok(Self(value.try_into()?))
    }
}

impl TryFrom<FileMode> for u32 {
    type Error = TryFromIntError;

    fn try_from(value: FileMode) -> Result<Self, Self::Error> {
        let FileMode(value) = value;
        value.try_into()
    }
}

impl TryFrom<i32> for FileMode {
    type Error = TryFromIntError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Ok(Self(value.try_into()?))
    }
}

impl TryFrom<FileMode> for i32 {
    type Error = TryFromIntError;

    fn try_from(value: FileMode) -> Result<Self, Self::Error> {
        let FileMode(value) = value;
        value.try_into()
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Tristate {
    False,
    Partial,
    True,
}

impl From<bool> for Tristate {
    fn from(value: bool) -> Self {
        match value {
            true => Tristate::True,
            false => Tristate::False,
        }
    }
}

/// A container of selected changes and commit metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Commit {
    /// The commit message. If `Some`, then the commit message will be previewed
    /// in the UI and the user will be able to edit it. If `None`, the commit
    /// message will not be shown or editable.
    pub message: Option<String>,
}

/// The state of a file to be recorded.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct File<'a> {
    /// The path to the previous version of the file, for display purposes. This
    /// should be set if the file was renamed or copied from another file.
    pub old_path: Option<Cow<'a, Path>>,

    /// The path to the current version of the file, for display purposes.
    pub path: Cow<'a, Path>,

    /// The Unix file mode of the file (before any changes), if available. This
    /// may be rendered by the UI.
    ///
    /// This value is not directly modified by the UI; instead, construct a
    /// [`Section::FileMode`] and use the [`FileState::get_file_mode`] function
    /// to read a user-provided updated to the file mode function to read a
    /// user-provided updated to the file mode.
    pub file_mode: Option<FileMode>,

    /// The set of [`Section`]s inside the file.
    pub sections: Vec<Section<'a>>,
}

/// The contents of a file selected as part of the record operation.
#[derive(Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Ord)]
pub enum SelectedContents<'a> {
    /// The file didn't exist or was deleted.
    Absent,

    /// The file contents have not changed.
    Unchanged,

    /// The file contains binary contents.
    Binary {
        /// The UI description of the old version of the file.
        old_description: Option<Cow<'a, str>>,
        /// The UI description of the new version of the file.
        new_description: Option<Cow<'a, str>>,
    },

    /// The file contained the following text contents.
    Present {
        /// The contents of the file.
        contents: String,
    },
}

impl SelectedContents<'_> {
    fn push_str(&mut self, s: &str) {
        match self {
            SelectedContents::Absent | SelectedContents::Unchanged => {
                *self = SelectedContents::Present {
                    contents: s.to_owned(),
                };
            }
            SelectedContents::Binary {
                old_description: _,
                new_description: _,
            } => {
                // Do nothing.
            }
            SelectedContents::Present { contents } => {
                contents.push_str(s);
            }
        }
    }
}

impl File<'_> {
    /// Get the new Unix file mode. If the user selected a
    /// [`Section::FileMode`], then returns that file mode. Otherwise, returns
    /// the `file_mode` value that this [`FileState`] was constructed with.
    pub fn get_file_mode(&self) -> Option<FileMode> {
        let Self {
            old_path: _,
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
                    is_checked: false,
                    before: _,
                    after: _,
                }
                | Section::Binary { .. } => None,

                Section::FileMode {
                    is_checked: true,
                    before: _,
                    after,
                } => Some(*after),
            })
            .or(*file_mode)
    }

    /// Calculate the `(selected, unselected)` contents of the file. For
    /// example, the first value would be suitable for staging or committing,
    /// and the second value would be suitable for potentially recording again.
    pub fn get_selected_contents(&self) -> (SelectedContents, SelectedContents) {
        let mut acc_selected = SelectedContents::Absent;
        let mut acc_unselected = SelectedContents::Absent;
        let Self {
            old_path: _,
            path: _,
            file_mode: _,
            sections,
        } = self;
        for section in sections {
            match section {
                Section::Unchanged { lines } => {
                    for line in lines {
                        acc_selected.push_str(line);
                        acc_unselected.push_str(line);
                    }
                }

                Section::Changed { lines } => {
                    for line in lines {
                        let SectionChangedLine {
                            is_checked,
                            change_type,
                            line,
                        } = line;
                        match (change_type, is_checked) {
                            (ChangeType::Added, true) | (ChangeType::Removed, false) => {
                                acc_selected.push_str(line);
                            }
                            (ChangeType::Added, false) | (ChangeType::Removed, true) => {
                                acc_unselected.push_str(line);
                            }
                        }
                    }
                }

                Section::FileMode {
                    is_checked,
                    before,
                    after,
                } => {
                    if *is_checked && after == &FileMode::absent() {
                        acc_selected = SelectedContents::Absent;
                    } else if !is_checked && before == &FileMode::absent() {
                        acc_unselected = SelectedContents::Absent;
                    }
                }

                Section::Binary {
                    is_checked,
                    old_description,
                    new_description,
                } => {
                    let selected_contents = SelectedContents::Binary {
                        old_description: old_description.clone(),
                        new_description: new_description.clone(),
                    };
                    if *is_checked {
                        acc_selected = selected_contents;
                        acc_unselected = SelectedContents::Unchanged;
                    } else {
                        acc_selected = SelectedContents::Unchanged;
                        acc_unselected = selected_contents;
                    }
                }
            }
        }
        (acc_selected, acc_unselected)
    }

    /// Get the tristate value of the file. If there are no sections in this
    /// file, returns `Tristate::False`.
    pub fn tristate(&self) -> Tristate {
        let Self {
            old_path: _,
            path: _,
            file_mode: _,
            sections,
        } = self;
        let mut seen_value = None;
        for section in sections {
            match section {
                Section::Unchanged { .. } => {}
                Section::Changed { lines } => {
                    for line in lines {
                        seen_value = match (seen_value, line.is_checked) {
                            (None, is_checked) => Some(is_checked),
                            (Some(true), true) => Some(true),
                            (Some(false), false) => Some(false),
                            (Some(true), false) | (Some(false), true) => return Tristate::Partial,
                        };
                    }
                }
                Section::FileMode {
                    is_checked,
                    before: _,
                    after: _,
                }
                | Section::Binary {
                    is_checked,
                    old_description: _,
                    new_description: _,
                } => {
                    seen_value = match (seen_value, is_checked) {
                        (None, is_checked) => Some(*is_checked),
                        (Some(true), true) => Some(true),
                        (Some(false), false) => Some(false),
                        (Some(true), false) | (Some(false), true) => return Tristate::Partial,
                    }
                }
            }
        }
        match seen_value {
            Some(true) => Tristate::True,
            None | Some(false) => Tristate::False,
        }
    }

    /// Set the selection of all sections and lines in this file.
    pub fn set_checked(&mut self, checked: bool) {
        let Self {
            old_path: _,
            path: _,
            file_mode: _,
            sections,
        } = self;
        for section in sections {
            section.set_checked(checked);
        }
    }

    /// Toggle the selection of all sections in this file.
    pub fn toggle_all(&mut self) {
        let Self {
            old_path: _,
            path: _,
            file_mode: _,
            sections,
        } = self;
        for section in sections {
            section.toggle_all();
        }
    }
}

/// A section of a file to be rendered and recorded.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum Section<'a> {
    /// This section of the file is unchanged and just used for context.
    ///
    /// By default, only part of the context will be shown. However, all of the
    /// context lines should be provided so that they can be used to globally
    /// number the lines correctly.
    Unchanged {
        /// The contents of the lines, including their trailing newline
        /// character(s), if any.
        lines: Vec<Cow<'a, str>>,
    },

    /// This section of the file is changed, and the user needs to select which
    /// specific changed lines to record.
    Changed {
        /// The contents of the lines, including their trailing newline
        /// character(s), if any.
        lines: Vec<SectionChangedLine<'a>>,
    },

    /// This indicates that the Unix file mode of the file changed, and that the
    /// user needs to accept that mode change or not. This is not part of the
    /// "contents" of the file per se, but it's rendered inline as if it were.
    FileMode {
        /// Whether or not the file mode change was selected for inclusion in
        /// the UI.
        is_checked: bool,

        /// The old file mode.
        before: FileMode,

        /// The new file mode.
        after: FileMode,
    },

    /// This file contains binary contents.
    Binary {
        /// Whether or not the binary contents change was selected for inclusion
        /// in the UI.
        is_checked: bool,

        /// The description of the old binary contents, for use in the UI only.
        old_description: Option<Cow<'a, str>>,

        /// The description of the new binary contents, for use in the UI only.
        new_description: Option<Cow<'a, str>>,
    },
}

impl Section<'_> {
    /// Whether or not this section contains user-editable content (as opposed
    /// to simply contextual content).
    pub fn is_editable(&self) -> bool {
        match self {
            Section::Unchanged { .. } => false,
            Section::Changed { .. } | Section::FileMode { .. } | Section::Binary { .. } => true,
        }
    }

    /// Get the tristate value of this section. If there are no items in this
    /// section, returns `Tristate::False`.
    pub fn tristate(&self) -> Tristate {
        let mut seen_value = None;
        match self {
            Section::Unchanged { .. } => {}
            Section::Changed { lines } => {
                for line in lines {
                    seen_value = match (seen_value, line.is_checked) {
                        (None, is_checked) => Some(is_checked),
                        (Some(true), true) => Some(true),
                        (Some(false), false) => Some(false),
                        (Some(true), false) | (Some(false), true) => return Tristate::Partial,
                    };
                }
            }
            Section::FileMode {
                is_checked,
                before: _,
                after: _,
            }
            | Section::Binary {
                is_checked,
                old_description: _,
                new_description: _,
            } => {
                seen_value = match (seen_value, is_checked) {
                    (None, is_checked) => Some(*is_checked),
                    (Some(true), true) => Some(true),
                    (Some(false), false) => Some(false),
                    (Some(true), false) | (Some(false), true) => return Tristate::Partial,
                }
            }
        }
        match seen_value {
            Some(true) => Tristate::True,
            None | Some(false) => Tristate::False,
        }
    }

    /// Select or unselect all items in this section.
    pub fn set_checked(&mut self, checked: bool) {
        match self {
            Section::Unchanged { .. } => {}
            Section::Changed { lines } => {
                for line in lines {
                    line.is_checked = checked;
                }
            }
            Section::FileMode { is_checked, .. } => {
                *is_checked = checked;
            }
            Section::Binary { is_checked, .. } => {
                *is_checked = checked;
            }
        }
    }

    /// Toggle the selection of this section.
    pub fn toggle_all(&mut self) {
        match self {
            Section::Unchanged { .. } => {}
            Section::Changed { lines } => {
                for line in lines {
                    line.is_checked = !line.is_checked;
                }
            }
            Section::FileMode { is_checked, .. } => {
                *is_checked = !*is_checked;
            }
            Section::Binary { is_checked, .. } => {
                *is_checked = !*is_checked;
            }
        }
    }
}

/// The type of change in the patch/diff.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum ChangeType {
    /// The line was added.
    Added,

    /// The line was removed.
    Removed,
}

/// A changed line inside a `Section`.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct SectionChangedLine<'a> {
    /// Whether or not this line was selected to be recorded.
    pub is_checked: bool,

    /// The type of change this line was.
    pub change_type: ChangeType,

    /// The contents of the line, including its trailing newline character(s),
    /// if any.
    pub line: Cow<'a, str>,
}
