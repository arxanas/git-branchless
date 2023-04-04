use std::{borrow::Cow, path::Path};

use assert_matches::assert_matches;
use scm_record::{
    ChangeType, Event, EventSource, File, RecordError, RecordState, Recorder, Section,
    SectionChangedLine, TestingScreenshot,
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
        6,
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
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
    "###);
    insta::assert_display_snapshot!(screenshot2, @r###"
    "       20 this is some text                                                     "
    "  (~) Section 1/1                                                               "
    "    [×] - before text 1                                                         "
    "    [×] - before text 2                                                         "
    "    [×] + after text 1                                                          "
    "    [ ] + after text 2                                                          "
    "###);
    insta::assert_display_snapshot!(screenshot3, @r###"
    "  [×] Section 1/1                                                               "
    "    [×] - before text 1                                                         "
    "    [×] - before text 2                                                         "
    "    [×] + after text 1                                                          "
    "    [×] + after text 2                                                          "
    "        5 this is some trailing text                                            "
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
    "        1 this is some text                                                                         "
    "        2 this is some text                                                                         "
    "        3 this is some text                                                                         "
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
    "        2 Some lead┌Quit───────────────────────────────────────────────────────┐                    "
    "  [×] Section 1/1  │You have changes to 2 files. Are you sure you want to quit?│                    "
    "    [×] - before te│                                                           │                    "
    "    [×] - before te│                                                           │                    "
    "    [×] + after tex│                                                           │                    "
    "    [×] + after tex│                                                           │                    "
    "        5 this is s│                                                           │                    "
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
    "        1┌Quit───────────────────────────────────────────────────────┐          "
    "        2│You have changes to 2 files. Are you sure you want to quit?│          "
    "        3└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
    "###);
    insta::assert_display_snapshot!(expect_c_does_nothing, @r###"
    "(~) foo/bar                                                                     "
    "        1┌Quit───────────────────────────────────────────────────────┐          "
    "        2│You have changes to 2 files. Are you sure you want to quit?│          "
    "        3└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
    "###);
    insta::assert_display_snapshot!(expect_q_closes_quit_dialog, @r###"
    "(~) foo/bar                                                                     "
    "        1 this is some text                                                     "
    "        2 this is some text                                                     "
    "        3 this is some text                                                     "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
    "###);
    insta::assert_display_snapshot!(expect_ctrl_c_opens_quit_dialog, @r###"
    "(~) foo/bar                                                                     "
    "        1┌Quit───────────────────────────────────────────────────────┐          "
    "        2│You have changes to 2 files. Are you sure you want to quit?│          "
    "        3└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
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
    "        1┌Quit───────────────────────────────────────────────────────┐          "
    "        2│You have changes to 2 files. Are you sure you want to quit?│          "
    "        3└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
    "###);
    insta::assert_display_snapshot!(expect_left_focuses_go_back_button, @r###"
    "(~) foo/bar                                                                     "
    "        1┌Quit───────────────────────────────────────────────────────┐          "
    "        2│You have changes to 2 files. Are you sure you want to quit?│          "
    "        3└───────────────────────────────────────────(Go Back)─[Quit]┘          "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
    "###);
    insta::assert_display_snapshot!(expect_left_again_does_not_wrap, @r###"
    "(~) foo/bar                                                                     "
    "        1┌Quit───────────────────────────────────────────────────────┐          "
    "        2│You have changes to 2 files. Are you sure you want to quit?│          "
    "        3└───────────────────────────────────────────(Go Back)─[Quit]┘          "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
    "###);
    insta::assert_display_snapshot!(expect_back_button_closes_quit_dialog, @r###"
    "(~) foo/bar                                                                     "
    "        1 this is some text                                                     "
    "        2 this is some text                                                     "
    "        3 this is some text                                                     "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
    "###);
    insta::assert_display_snapshot!(expect_right_focuses_quit_button, @r###"
    "(~) foo/bar                                                                     "
    "        1┌Quit───────────────────────────────────────────────────────┐          "
    "        2│You have changes to 2 files. Are you sure you want to quit?│          "
    "        3└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
    "###);
    insta::assert_display_snapshot!(expect_right_again_does_not_wrap, @r###"
    "(~) foo/bar                                                                     "
    "        1┌Quit───────────────────────────────────────────────────────┐          "
    "        2│You have changes to 2 files. Are you sure you want to quit?│          "
    "        3└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "        ⋮                                                                       "
    "       18 this is some text                                                     "
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
                    before: 0o100644,
                    after: 0o100755,
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
                        before: 33188,
                        after: 33261,
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
