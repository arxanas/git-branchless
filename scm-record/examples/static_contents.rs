#![warn(clippy::all, clippy::as_conversions)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

use std::borrow::Cow;
use std::path::PathBuf;

use scm_record::{FileState, RecordError, RecordState, Recorder, Section, SectionChangedLine};

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

    let recorder = Recorder::new(record_state);
    let result = recorder.run();
    match result {
        Ok(result) => {
            let RecordState { file_states } = result;
            for (path, file_state) in file_states {
                println!("--- Path {path:?} final contents: ---");
                let (selected, _unselected) = file_state.get_selected_contents();
                print!("{selected}");
            }
        }
        Err(RecordError::Cancelled) => println!("Cancelled!"),
        Err(err) => {
            println!("Error: {err}");
        }
    }
}
