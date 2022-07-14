#![warn(clippy::all, clippy::as_conversions)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

use std::borrow::Cow;
use std::io;
use std::path::PathBuf;

use cursive::backends::crossterm;
use cursive::CursiveRunnable;
use cursive_buffered_backend::BufferedBackend;

use git_record::Recorder;
use git_record::{FileState, RecordError, RecordState, Section, SectionChangedLine};

fn main() {
    let file_states = vec![
        (
            PathBuf::from("foo/bar"),
            FileState {
                file_mode: None,
                sections: vec![
                    Section::Unchanged {
                        contents: std::iter::repeat(Cow::Borrowed("this is some text"))
                            .take(20)
                            .collect(),
                    },
                    Section::Changed {
                        before: vec![
                            SectionChangedLine {
                                is_selected: true,
                                line: Cow::Borrowed("before text 1"),
                            },
                            SectionChangedLine {
                                is_selected: true,
                                line: Cow::Borrowed("before text 2"),
                            },
                        ],
                        after: vec![
                            SectionChangedLine {
                                is_selected: true,
                                line: Cow::Borrowed("after text 1"),
                            },
                            SectionChangedLine {
                                is_selected: false,
                                line: Cow::Borrowed("after text 2"),
                            },
                        ],
                    },
                    Section::Unchanged {
                        contents: vec![Cow::Borrowed("this is some trailing text")],
                    },
                ],
            },
        ),
        (
            PathBuf::from("baz"),
            FileState {
                file_mode: None,
                sections: vec![
                    Section::Unchanged {
                        contents: vec![
                            Cow::Borrowed("Some leading text 1"),
                            Cow::Borrowed("Some leading text 2"),
                        ],
                    },
                    Section::Changed {
                        before: vec![
                            SectionChangedLine {
                                is_selected: true,
                                line: Cow::Borrowed("before text 1"),
                            },
                            SectionChangedLine {
                                is_selected: true,
                                line: Cow::Borrowed("before text 2"),
                            },
                        ],
                        after: vec![
                            SectionChangedLine {
                                is_selected: true,
                                line: Cow::Borrowed("after text 1"),
                            },
                            SectionChangedLine {
                                is_selected: true,
                                line: Cow::Borrowed("after text 2"),
                            },
                        ],
                    },
                    Section::Unchanged {
                        contents: vec![Cow::Borrowed("this is some trailing text")],
                    },
                ],
            },
        ),
    ];
    let record_state = RecordState { file_states };

    let siv = CursiveRunnable::new(|| -> io::Result<_> {
        // Use crossterm to ensure that we support Windows.
        let crossterm_backend = crossterm::Backend::init()?;
        Ok(Box::new(BufferedBackend::new(crossterm_backend)))
    });
    let siv = siv.into_runner();

    let recorder = Recorder::new(record_state);
    let result = recorder.run(siv);
    match result {
        Ok(result) => {
            let RecordState { file_states } = result;
            let mut is_first = true;
            for (path, file_state) in file_states {
                if is_first {
                    is_first = false;
                } else {
                    println!();
                }

                println!("Path {} will have these final contents:", path.display());
                let (selected, _unselected) = file_state.get_selected_contents();
                print!("{}", selected);
            }
        }
        Err(RecordError::Cancelled) => println!("Cancelled!"),
    };
}
