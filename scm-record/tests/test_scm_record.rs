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
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "    [×] - before text 1                                                         "
    "    [×] - before text 2                                                         "
    "###);
    insta::assert_display_snapshot!(screenshot2, @r###"
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "  (~) Section 1/1                                                               "
    "    [×] - before text 1                                                         "
    "    [×] - before text 2                                                         "
    "    [×] + after text 1                                                          "
    "    [ ] + after text 2                                                          "
    "###);
    insta::assert_display_snapshot!(screenshot3, @r###"
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "          this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "    [×] - before text 1                                                         "
    "    [×] - before text 2                                                         "
    "    [×] + after text 1                                                          "
    "    [ ] + after text 2                                                          "
    "          this is some trailing text                                            "
    "(×) baz                                                                         "
    "          Some leading text 1                                                   "
    "          Some leading text 2                                                   "
    "  [×] Section 1/1                                                               "
    "    [×] - before text 1                                                         "
    "    [×] - before text 2                                                         "
    "    [×] + after text 1                                                          "
    "    [×] + after text 2                                                          "
    "          this is some trailing text                                            "
    "###);
    Ok(())
}
