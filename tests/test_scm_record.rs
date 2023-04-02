use std::borrow::Cow;
use std::path::Path;

use assert_matches::assert_matches;
use scm_record::{
    ChangeType, Event, EventSource, File, FileMode, RecordError, RecordState, Recorder, Section,
    SectionChangedLine, TestingScreenshot,
};

fn example_contents() -> RecordState<'static> {
    let example_contents = include_str!("example_contents.json");
    serde_json::from_str(example_contents).unwrap()
}

#[test]
fn test_select_scroll_into_view() -> eyre::Result<()> {
    let initial = TestingScreenshot::default();
    let scroll_to_first_section = TestingScreenshot::default();
    let scroll_to_second_file = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        5,
        [
            initial.event(),
            // Scroll to first section (off-screen).
            Event::FocusNext,
            scroll_to_first_section.event(),
            // Scroll to second file (off-screen). It should display the entire
            // file contents, since they all fit in the viewport.
            Event::FocusNext,
            Event::FocusNext,
            Event::FocusNext,
            Event::FocusNext,
            Event::FocusNext,
            scroll_to_second_file.event(),
            Event::QuitAccept,
        ],
    );
    let state = example_contents();
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "(~) foo/bar                                                                     "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
    "       19 this is some text                                                     "
    "       20 this is some text                                                     "
    "###);
    insta::assert_display_snapshot!(scroll_to_first_section, @r###"
    "  (~) Section 1/1                                                               "
    "    [×] - before text 1                                                         "
    "    [×] - before text 2                                                         "
    "    [×] + after text 1                                                          "
    "    [ ] + after text 2                                                          "
    "###);
    insta::assert_display_snapshot!(scroll_to_second_file, @r###"
    "(×) baz                                                                         "
    "        1 Some leading text 1                                                   "
    "        2 Some leading text 2                                                   "
    "  [×] Section 1/1                                                               "
    "    [×] - before text 1                                                         "
    "###);
    Ok(())
}

#[test]
fn test_quit_dialog_size() -> eyre::Result<()> {
    let expect_quit_dialog_to_be_centered = TestingScreenshot::default();
    let event_source = EventSource::testing(
        100,
        40,
        [
            Event::QuitInterrupt,
            expect_quit_dialog_to_be_centered.event(),
            Event::QuitInterrupt,
        ],
    );
    let state = example_contents();
    let recorder = Recorder::new(state, event_source);
    let result = recorder.run();
    assert_matches!(result, Err(RecordError::Cancelled));
    insta::assert_display_snapshot!(expect_quit_dialog_to_be_centered, @r###"
    "(~) foo/bar                                                                                         "
    "        ⋮                                                                                           "
    "       18 this is some text                                                                         "
    "       19 this is some text                                                                         "
    "       20 this is some text                                                                         "
    "  [~] Section 1/1                                                                                   "
    "    [×] - before text 1                                                                             "
    "    [×] - before text 2                                                                             "
    "    [×] + after text 1                                                                              "
    "    [ ] + after text 2                                                                              "
    "       23 this is some trailing text                                                                "
    "[×] baz                                                                                             "
    "        1 Some leading text 1                                                                       "
    "        2 Some leading text 2                                                                       "
    "  [×] Section 1/1                                                                                   "
    "    [×] - before text 1                                                                             "
    "    [×] - before te┌Quit───────────────────────────────────────────────────────┐                    "
    "    [×] + after tex│You have changes to 2 files. Are you sure you want to quit?│                    "
    "    [×] + after tex│                                                           │                    "
    "        5 this is s│                                                           │                    "
    "                   │                                                           │                    "
    "                   │                                                           │                    "
    "                   │                                                           │                    "
    "                   └───────────────────────────────────────────[Go Back]─(Quit)┘                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "                                                                                                    "
    "###);
    Ok(())
}

#[test]
fn test_quit_dialog_keyboard_navigation() -> eyre::Result<()> {
    let expect_q_opens_quit_dialog = TestingScreenshot::default();
    let expect_c_does_nothing = TestingScreenshot::default();
    let expect_q_closes_quit_dialog = TestingScreenshot::default();
    let expect_ctrl_c_opens_quit_dialog = TestingScreenshot::default();
    let expect_exited = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        6,
        [
            // Pressing 'q' should display the quit dialog.
            Event::QuitCancel,
            expect_q_opens_quit_dialog.event(),
            // Pressing 'c' now should do nothing.
            Event::QuitAccept,
            expect_c_does_nothing.event(),
            // Pressing 'q' now should close the quit dialog.
            Event::QuitCancel,
            expect_q_closes_quit_dialog.event(),
            // Pressing ctrl-c should display the quit dialog.
            Event::QuitInterrupt,
            expect_ctrl_c_opens_quit_dialog.event(),
            // Pressing ctrl-c again should exit.
            Event::QuitInterrupt,
            expect_exited.event(),
        ],
    );
    let state = example_contents();
    let recorder = Recorder::new(state, event_source);
    assert_matches!(recorder.run(), Err(RecordError::Cancelled));
    insta::assert_display_snapshot!(expect_q_opens_quit_dialog, @r###"
    "(~) foo/bar                                                                     "
    "        ⋮┌Quit───────────────────────────────────────────────────────┐          "
    "       18│You have changes to 2 files. Are you sure you want to quit?│          "
    "       19└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "       20 this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "###);
    insta::assert_display_snapshot!(expect_c_does_nothing, @r###"
    "(~) foo/bar                                                                     "
    "        ⋮┌Quit───────────────────────────────────────────────────────┐          "
    "       18│You have changes to 2 files. Are you sure you want to quit?│          "
    "       19└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "       20 this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "###);
    insta::assert_display_snapshot!(expect_q_closes_quit_dialog, @r###"
    "(~) foo/bar                                                                     "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
    "       19 this is some text                                                     "
    "       20 this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "###);
    insta::assert_display_snapshot!(expect_ctrl_c_opens_quit_dialog, @r###"
    "(~) foo/bar                                                                     "
    "        ⋮┌Quit───────────────────────────────────────────────────────┐          "
    "       18│You have changes to 2 files. Are you sure you want to quit?│          "
    "       19└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "       20 this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "###);
    insta::assert_display_snapshot!(expect_exited, @"<this screenshot was never assigned>");
    Ok(())
}

#[test]
fn test_quit_dialog_buttons() -> eyre::Result<()> {
    let expect_quit_button_focused_initially = TestingScreenshot::default();
    let expect_left_focuses_go_back_button = TestingScreenshot::default();
    let expect_left_again_does_not_wrap = TestingScreenshot::default();
    let expect_back_button_closes_quit_dialog = TestingScreenshot::default();
    let expect_right_focuses_quit_button = TestingScreenshot::default();
    let expect_right_again_does_not_wrap = TestingScreenshot::default();
    let expect_exited = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        6,
        [
            Event::QuitCancel,
            expect_quit_button_focused_initially.event(),
            // Pressing left should select the back button.
            Event::FocusOuter,
            expect_left_focuses_go_back_button.event(),
            // Pressing left again should do nothing.
            Event::FocusOuter,
            expect_left_again_does_not_wrap.event(),
            // Selecting the back button should close the dialog.
            Event::ToggleItem,
            expect_back_button_closes_quit_dialog.event(),
            Event::QuitCancel,
            // Pressing right should select the quit button.
            Event::FocusOuter,
            Event::FocusInner,
            expect_right_focuses_quit_button.event(),
            // Pressing right again should do nothing.
            Event::FocusInner,
            expect_right_again_does_not_wrap.event(),
            // Selecting the quit button should quit.
            Event::ToggleItem,
            expect_exited.event(),
        ],
    );
    let state = example_contents();
    let recorder = Recorder::new(state, event_source);
    assert_matches!(recorder.run(), Err(RecordError::Cancelled));
    insta::assert_display_snapshot!(expect_quit_button_focused_initially, @r###"
    "(~) foo/bar                                                                     "
    "        ⋮┌Quit───────────────────────────────────────────────────────┐          "
    "       18│You have changes to 2 files. Are you sure you want to quit?│          "
    "       19└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "       20 this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "###);
    insta::assert_display_snapshot!(expect_left_focuses_go_back_button, @r###"
    "(~) foo/bar                                                                     "
    "        ⋮┌Quit───────────────────────────────────────────────────────┐          "
    "       18│You have changes to 2 files. Are you sure you want to quit?│          "
    "       19└───────────────────────────────────────────(Go Back)─[Quit]┘          "
    "       20 this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "###);
    insta::assert_display_snapshot!(expect_left_again_does_not_wrap, @r###"
    "(~) foo/bar                                                                     "
    "        ⋮┌Quit───────────────────────────────────────────────────────┐          "
    "       18│You have changes to 2 files. Are you sure you want to quit?│          "
    "       19└───────────────────────────────────────────(Go Back)─[Quit]┘          "
    "       20 this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "###);
    insta::assert_display_snapshot!(expect_back_button_closes_quit_dialog, @r###"
    "(~) foo/bar                                                                     "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
    "       19 this is some text                                                     "
    "       20 this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "###);
    insta::assert_display_snapshot!(expect_right_focuses_quit_button, @r###"
    "(~) foo/bar                                                                     "
    "        ⋮┌Quit───────────────────────────────────────────────────────┐          "
    "       18│You have changes to 2 files. Are you sure you want to quit?│          "
    "       19└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "       20 this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "###);
    insta::assert_display_snapshot!(expect_right_again_does_not_wrap, @r###"
    "(~) foo/bar                                                                     "
    "        ⋮┌Quit───────────────────────────────────────────────────────┐          "
    "       18│You have changes to 2 files. Are you sure you want to quit?│          "
    "       19└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "       20 this is some text                                                     "
    "  [~] Section 1/1                                                               "
    "###);
    insta::assert_display_snapshot!(expect_exited, @"<this screenshot was never assigned>");
    Ok(())
}

#[test]
fn test_enter_next() -> eyre::Result<()> {
    let state = RecordState {
        files: vec![
            File {
                path: Cow::Borrowed(Path::new("foo")),
                file_mode: None,
                sections: vec![Section::Changed {
                    lines: vec![
                        SectionChangedLine {
                            is_toggled: false,
                            change_type: ChangeType::Added,
                            line: Cow::Borrowed("world"),
                        },
                        SectionChangedLine {
                            is_toggled: false,
                            change_type: ChangeType::Removed,
                            line: Cow::Borrowed("hello"),
                        },
                    ],
                }],
            },
            File {
                path: Cow::Borrowed(Path::new("bar")),
                file_mode: None,
                sections: vec![Section::Changed {
                    lines: vec![
                        SectionChangedLine {
                            is_toggled: false,
                            change_type: ChangeType::Added,
                            line: Cow::Borrowed("world"),
                        },
                        SectionChangedLine {
                            is_toggled: false,
                            change_type: ChangeType::Removed,
                            line: Cow::Borrowed("hello"),
                        },
                    ],
                }],
            },
        ],
    };

    let first_file_selected = TestingScreenshot::default();
    let second_file_selected = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        6,
        [
            Event::ToggleItemAndAdvance,
            first_file_selected.event(),
            Event::ToggleItemAndAdvance,
            second_file_selected.event(),
            Event::QuitCancel,
            Event::ToggleItemAndAdvance,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    assert_matches!(recorder.run(), Err(RecordError::Cancelled));
    insta::assert_display_snapshot!(first_file_selected, @r###"
    "[×] foo                                                                         "
    "  [×] Section 1/1                                                               "
    "    [×] + world                                                                 "
    "    [×] - hello                                                                 "
    "( ) bar                                                                         "
    "  [ ] Section 1/1                                                               "
    "###);
    insta::assert_display_snapshot!(second_file_selected, @r###"
    "    [×] + world                                                                 "
    "    [×] - hello                                                                 "
    "(×) bar                                                                         "
    "  [×] Section 1/1                                                               "
    "    [×] + world                                                                 "
    "    [×] - hello                                                                 "
    "###);
    Ok(())
}

#[test]
fn test_file_mode_change() -> eyre::Result<()> {
    let state = RecordState {
        files: vec![
            File {
                path: Cow::Borrowed(Path::new("foo")),
                file_mode: None,
                sections: vec![],
            },
            File {
                path: Cow::Borrowed(Path::new("bar")),
                file_mode: None,
                sections: vec![Section::FileMode {
                    is_toggled: false,
                    before: FileMode(0o100644),
                    after: FileMode(0o100755),
                }],
            },
            File {
                path: Cow::Borrowed(Path::new("qux")),
                file_mode: None,
                sections: vec![],
            },
        ],
    };

    let before_toggle = TestingScreenshot::default();
    let after_toggle = TestingScreenshot::default();
    let expect_no_crash = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        6,
        [
            before_toggle.event(),
            Event::FocusNext,
            Event::FocusNext,
            Event::ToggleItem,
            after_toggle.event(),
            Event::FocusNext,
            expect_no_crash.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    insta::assert_debug_snapshot!(recorder.run()?, @r###"
    RecordState {
        files: [
            File {
                path: "foo",
                file_mode: None,
                sections: [],
            },
            File {
                path: "bar",
                file_mode: None,
                sections: [
                    FileMode {
                        is_toggled: true,
                        before: FileMode(
                            33188,
                        ),
                        after: FileMode(
                            33261,
                        ),
                    },
                ],
            },
            File {
                path: "qux",
                file_mode: None,
                sections: [],
            },
        ],
    }
    "###);
    insta::assert_display_snapshot!(before_toggle, @r###"
    "( ) foo                                                                         "
    "[ ] bar                                                                         "
    "  [ ] File mode changed from 100644 to 100755                                   "
    "[ ] qux                                                                         "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_toggle, @r###"
    "[ ] foo                                                                         "
    "[×] bar                                                                         "
    "  (×) File mode changed from 100644 to 100755                                   "
    "[ ] qux                                                                         "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(expect_no_crash, @r###"
    "[ ] foo                                                                         "
    "[×] bar                                                                         "
    "  [×] File mode changed from 100644 to 100755                                   "
    "( ) qux                                                                         "
    "                                                                                "
    "                                                                                "
    "###);
    Ok(())
}

#[test]
fn test_abbreviate_unchanged_sections() -> eyre::Result<()> {
    let num_context_lines = 3;
    let section_length = num_context_lines * 2;
    let middle_length = section_length + 1;
    let state = RecordState {
        files: vec![File {
            path: Cow::Borrowed(Path::new("foo")),
            file_mode: None,
            sections: vec![
                Section::Unchanged {
                    lines: (1..=section_length)
                        .map(|x| Cow::Owned(format!("start line {x}/{section_length}\n")))
                        .collect(),
                },
                Section::Changed {
                    lines: vec![SectionChangedLine {
                        is_toggled: false,
                        change_type: ChangeType::Added,
                        line: Cow::Borrowed("changed\n"),
                    }],
                },
                Section::Unchanged {
                    lines: (1..=middle_length)
                        .map(|x| Cow::Owned(format!("middle line {x}/{middle_length}\n")))
                        .collect(),
                },
                Section::Changed {
                    lines: vec![SectionChangedLine {
                        is_toggled: false,
                        change_type: ChangeType::Added,
                        line: Cow::Borrowed("changed\n"),
                    }],
                },
                Section::Unchanged {
                    lines: (1..=section_length)
                        .map(|x| Cow::Owned(format!("end line {x}/{section_length}\n")))
                        .collect(),
                },
            ],
        }],
    };

    let screenshot = TestingScreenshot::default();
    let event_source = EventSource::testing(80, 24, [screenshot.event(), Event::QuitAccept]);
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;
    insta::assert_display_snapshot!(screenshot, @r###"
    "( ) foo                                                                         "
    "        ⋮                                                                       "
    "        4 start line 4/6                                                        "
    "        5 start line 5/6                                                        "
    "        6 start line 6/6                                                        "
    "  [ ] Section 1/2                                                               "
    "    [ ] + changed                                                               "
    "        7 middle line 1/7                                                       "
    "        8 middle line 2/7                                                       "
    "        9 middle line 3/7                                                       "
    "        ⋮                                                                       "
    "       11 middle line 5/7                                                       "
    "       12 middle line 6/7                                                       "
    "       13 middle line 7/7                                                       "
    "  [ ] Section 2/2                                                               "
    "    [ ] + changed                                                               "
    "       14 end line 1/6                                                          "
    "       15 end line 2/6                                                          "
    "       16 end line 3/6                                                          "
    "        ⋮                                                                       "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);

    Ok(())
}

#[test]
fn test_no_abbreviate_short_unchanged_sections() -> eyre::Result<()> {
    let num_context_lines = 3;
    let section_length = num_context_lines - 1;
    let middle_length = num_context_lines * 2;
    let state = RecordState {
        files: vec![File {
            path: Cow::Borrowed(Path::new("foo")),
            file_mode: None,
            sections: vec![
                Section::Unchanged {
                    lines: (1..=section_length)
                        .map(|x| Cow::Owned(format!("start line {x}/{section_length}\n")))
                        .collect(),
                },
                Section::Changed {
                    lines: vec![SectionChangedLine {
                        is_toggled: false,
                        change_type: ChangeType::Added,
                        line: Cow::Borrowed("changed\n"),
                    }],
                },
                Section::Unchanged {
                    lines: (1..=middle_length)
                        .map(|x| Cow::Owned(format!("middle line {x}/{middle_length}\n")))
                        .collect(),
                },
                Section::Changed {
                    lines: vec![SectionChangedLine {
                        is_toggled: false,
                        change_type: ChangeType::Added,
                        line: Cow::Borrowed("changed\n"),
                    }],
                },
                Section::Unchanged {
                    lines: (1..=section_length)
                        .map(|x| Cow::Owned(format!("end line {x}/{section_length}\n")))
                        .collect(),
                },
            ],
        }],
    };

    let screenshot = TestingScreenshot::default();
    let event_source = EventSource::testing(80, 20, [screenshot.event(), Event::QuitAccept]);
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;
    insta::assert_display_snapshot!(screenshot, @r###"
    "( ) foo                                                                         "
    "        1 start line 1/2                                                        "
    "        2 start line 2/2                                                        "
    "  [ ] Section 1/2                                                               "
    "    [ ] + changed                                                               "
    "        3 middle line 1/6                                                       "
    "        4 middle line 2/6                                                       "
    "        5 middle line 3/6                                                       "
    "        6 middle line 4/6                                                       "
    "        7 middle line 5/6                                                       "
    "        8 middle line 6/6                                                       "
    "  [ ] Section 2/2                                                               "
    "    [ ] + changed                                                               "
    "        9 end line 1/2                                                          "
    "       10 end line 2/2                                                          "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);

    Ok(())
}
