#![warn(clippy::all, clippy::as_conversions)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

mod cursive_utils;
mod tristate;
mod ui;

use std::io;
use std::path::PathBuf;

use cursive::backends::crossterm;
use cursive::CursiveRunnable;
use cursive_buffered_backend::BufferedBackend;
use cursive_utils::EventDrivenCursiveAppExt;
use ui::Recorder;

#[derive(Clone, Debug)]
pub struct HunkChangedLine<'a> {
    is_selected: bool,
    line: &'a str,
}

#[derive(Clone, Debug)]
pub enum Hunk<'a> {
    Unchanged {
        contents: &'a str,
    },
    Changed {
        before: Vec<HunkChangedLine<'a>>,
        after: Vec<HunkChangedLine<'a>>,
    },
}

#[derive(Clone, Debug)]
pub struct FileHunks<'a> {
    hunks: Vec<Hunk<'a>>,
}

impl FileHunks<'_> {
    pub fn get_selected_contents(&self) -> (String, String) {
        let mut acc_selected = String::new();
        let mut acc_unselected = String::new();
        for hunk in &self.hunks {
            match hunk {
                Hunk::Unchanged { contents } => {
                    acc_selected.push_str(contents);
                    acc_unselected.push_str(contents);
                }
                Hunk::Changed { before, after } => {
                    for HunkChangedLine { is_selected, line } in before {
                        // Note the inverted condition here.
                        if !*is_selected {
                            acc_selected.push_str(line);
                            acc_selected.push('\n');
                        } else {
                            acc_unselected.push_str(line);
                            acc_unselected.push('\n');
                        }
                    }

                    for HunkChangedLine { is_selected, line } in after {
                        if *is_selected {
                            acc_selected.push_str(line);
                            acc_selected.push('\n');
                        } else {
                            acc_unselected.push_str(line);
                            acc_unselected.push('\n');
                        }
                    }
                }
            }
        }
        (acc_selected, acc_unselected)
    }
}

#[derive(Clone, Debug)]
pub struct RecordState<'a> {
    files: Vec<(PathBuf, FileHunks<'a>)>,
}

#[derive(Clone, Debug)]
pub enum RecordError {
    Cancelled,
}

fn main() {
    let preamble = "this is some text\n".repeat(20);
    let files = vec![
        (
            PathBuf::from("foo/bar"),
            FileHunks {
                hunks: vec![
                    Hunk::Unchanged {
                        contents: &preamble,
                    },
                    Hunk::Changed {
                        before: vec![
                            HunkChangedLine {
                                is_selected: true,
                                line: "before text 1",
                            },
                            HunkChangedLine {
                                is_selected: true,
                                line: "before text 2",
                            },
                        ],
                        after: vec![
                            HunkChangedLine {
                                is_selected: true,
                                line: "after text 1",
                            },
                            HunkChangedLine {
                                is_selected: false,
                                line: "after text 2",
                            },
                        ],
                    },
                    Hunk::Unchanged {
                        contents: "this is some trailing text\n",
                    },
                ],
            },
        ),
        (
            PathBuf::from("baz"),
            FileHunks {
                hunks: vec![
                    Hunk::Unchanged {
                        contents: "Some leading text 1\nSome leading text 2\n",
                    },
                    Hunk::Changed {
                        before: vec![
                            HunkChangedLine {
                                is_selected: true,
                                line: "before text 1",
                            },
                            HunkChangedLine {
                                is_selected: true,
                                line: "before text 2",
                            },
                        ],
                        after: vec![
                            HunkChangedLine {
                                is_selected: true,
                                line: "after text 1",
                            },
                            HunkChangedLine {
                                is_selected: true,
                                line: "after text 2",
                            },
                        ],
                    },
                    Hunk::Unchanged {
                        contents: "this is some trailing text\n",
                    },
                ],
            },
        ),
    ];
    let record_state = RecordState { files };

    // TODO: let user select backend
    // let mut siv = cursive::default();
    let siv = CursiveRunnable::new(|| -> io::Result<_> {
        // Use crossterm to ensure that we support Windows.
        let crossterm_backend = crossterm::Backend::init()?;
        Ok(Box::new(BufferedBackend::new(crossterm_backend)))
    });
    let siv = siv.into_runner();

    let recorder = Recorder::new(record_state);
    let result = recorder.run(siv);
    let RecordState { files: result } = match result {
        Ok(result) => result,
        Err(RecordError::Cancelled) => todo!("Cancelled"),
    };
    for (path, file_hunks) in result {
        println!("Path {}", path.display());
        let (selected, _unselected) = file_hunks.get_selected_contents();
        print!("{}", selected);
    }
}
