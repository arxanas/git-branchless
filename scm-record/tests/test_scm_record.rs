use std::{borrow::Cow, path::Path};

use scm_record::{
    ChangeType, Event, EventSource, File, RecordState, Recorder, Section, SectionChangedLine,
    TestingScreenshot,
};

fn example_contents() -> RecordState<'static> {
    RecordState {
        files: vec![
            File {
                path: Cow::Borrowed(Path::new("foo/bar")),
                file_mode: None,
                sections: vec![
                    Section::Unchanged {
                        lines: std::iter::repeat(Cow::Borrowed("this is some text"))
                            .take(20)
                            .collect(),
                    },
                    Section::Changed {
                        lines: vec![
                            SectionChangedLine {
                                is_toggled: true,
                                change_type: ChangeType::Removed,
                                line: Cow::Borrowed("before text 1"),
                            },
                            SectionChangedLine {
                                is_toggled: true,
                                change_type: ChangeType::Removed,
                                line: Cow::Borrowed("before text 2"),
                            },
                            SectionChangedLine {
                                is_toggled: true,
                                change_type: ChangeType::Added,

                                line: Cow::Borrowed("after text 1"),
                            },
                            SectionChangedLine {
                                is_toggled: false,
                                change_type: ChangeType::Added,
                                line: Cow::Borrowed("after text 2"),
                            },
                        ],
                    },
                    Section::Unchanged {
                        lines: vec![Cow::Borrowed("this is some trailing text")],
                    },
                ],
            },
            File {
                path: Cow::Borrowed(Path::new("baz")),
                file_mode: None,
                sections: vec![
                    Section::Unchanged {
                        lines: vec![
                            Cow::Borrowed("Some leading text 1"),
                            Cow::Borrowed("Some leading text 2"),
                        ],
                    },
                    Section::Changed {
                        lines: vec![
                            SectionChangedLine {
                                is_toggled: true,
                                change_type: ChangeType::Removed,
                                line: Cow::Borrowed("before text 1"),
                            },
                            SectionChangedLine {
                                is_toggled: true,
                                change_type: ChangeType::Removed,
                                line: Cow::Borrowed("before text 2"),
                            },
                            SectionChangedLine {
                                is_toggled: true,
                                change_type: ChangeType::Added,
                                line: Cow::Borrowed("after text 1"),
                            },
                            SectionChangedLine {
                                is_toggled: true,
                                change_type: ChangeType::Added,
                                line: Cow::Borrowed("after text 2"),
                            },
                        ],
                    },
                    Section::Unchanged {
                        lines: vec![Cow::Borrowed("this is some trailing text")],
                    },
                ],
            },
        ],
    }
}

#[test]
fn test_select_scroll_into_view() -> eyre::Result<()> {
    let screenshot1 = TestingScreenshot::default();
    let screenshot2 = TestingScreenshot::default();
    let screenshot3 = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        24,
        [
            screenshot1.event(),
            // Scroll to first section (off-screen).
            Event::FocusNext,
            screenshot2.event(),
            // Scroll to second file (off-screen). It should display the entire
            // file contents, since they all fit in the viewport.
            Event::FocusNext,
            Event::FocusNext,
            Event::FocusNext,
            Event::FocusNext,
            Event::FocusNext,
            screenshot3.event(),
            Event::QuitAccept,
        ],
    );
    let state = example_contents();
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;
    insta::assert_display_snapshot!(screenshot1, @r###"
    "(~) foo/bar                                                                     "
    "        1 this is some text                                                     "
    "        2 this is some text                                                     "
    "        3 this is some text                                                     "
    "        4 this is some text                                                     "
    "        5 this is some text                                                     "
    "        6 this is some text                                                     "
    "        7 this is some text                                                     "
    "        8 this is some text                                                     "
    "        9 this is some text                                                     "
    "       10 this is some text                                                     "
    "       11 this is some text                                                     "
    "       12 this is some text                                                     "
    "       13 this is some text                                                     "
    "       14 this is some text                                                     "
    "       15 this is some text                                                     "
    "       16 this is some text                                                     "
    "       17 this is some text                                                     "
    "       18 this is some text                                                     "
    "       19 this is some text                                                     "
    "       20 this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "    [×] - before text 1                                                         "
    "    [×] - before text 2                                                         "
    "###);
    insta::assert_display_snapshot!(screenshot2, @r###"
    "        2 this is some text                                                     "
    "        3 this is some text                                                     "
    "        4 this is some text                                                     "
    "        5 this is some text                                                     "
    "        6 this is some text                                                     "
    "        7 this is some text                                                     "
    "        8 this is some text                                                     "
    "        9 this is some text                                                     "
    "       10 this is some text                                                     "
    "       11 this is some text                                                     "
    "       12 this is some text                                                     "
    "       13 this is some text                                                     "
    "       14 this is some text                                                     "
    "       15 this is some text                                                     "
    "       16 this is some text                                                     "
    "       17 this is some text                                                     "
    "       18 this is some text                                                     "
    "       19 this is some text                                                     "
    "       20 this is some text                                                     "
    "  (~) Section 1/1                                                               "
    "    [×] - before text 1                                                         "
    "    [×] - before text 2                                                         "
    "    [×] + after text 1                                                          "
    "    [ ] + after text 2                                                          "
    "###);
    insta::assert_display_snapshot!(screenshot3, @r###"
    "       12 this is some text                                                     "
    "       13 this is some text                                                     "
    "       14 this is some text                                                     "
    "       15 this is some text                                                     "
    "       16 this is some text                                                     "
    "       17 this is some text                                                     "
    "       18 this is some text                                                     "
    "       19 this is some text                                                     "
    "       20 this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "    [×] - before text 1                                                         "
    "    [×] - before text 2                                                         "
    "    [×] + after text 1                                                          "
    "    [ ] + after text 2                                                          "
    "       23 this is some trailing text                                            "
    "(×) baz                                                                         "
    "        1 Some leading text 1                                                   "
    "        2 Some leading text 2                                                   "
    "  [×] Section 1/1                                                               "
    "    [×] - before text 1                                                         "
    "    [×] - before text 2                                                         "
    "    [×] + after text 1                                                          "
    "    [×] + after text 2                                                          "
    "        5 this is some trailing text                                            "
    "###);
    Ok(())
}
