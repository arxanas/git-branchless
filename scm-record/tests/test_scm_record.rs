use std::borrow::Cow;
use std::path::Path;

use assert_matches::assert_matches;
use insta::{assert_debug_snapshot, assert_snapshot};
use scm_record::{
    helpers::make_binary_description, ChangeType, Event, EventSource, File, FileMode, RecordError,
    RecordState, Recorder, Section, SectionChangedLine, TestingScreenshot,
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
        6,
        [
            Event::ExpandAll,
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
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "###);
    insta::assert_display_snapshot!(scroll_to_first_section, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[~] foo/bar                                                                  [-]"
    "  (~) Section 1/1                                                            (-)"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "###);
    insta::assert_display_snapshot!(scroll_to_second_file, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(×) baz                                                                      (-)"
    "        1 Some leading text 1⏎                                                  "
    "        2 Some leading text 2⏎                                                  "
    "  [×] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "###);
    Ok(())
}

#[test]
fn test_toggle_all() -> eyre::Result<()> {
    let before = TestingScreenshot::default();
    let after = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        20,
        [
            Event::ExpandAll,
            before.event(),
            Event::ToggleAll,
            after.event(),
            Event::QuitAccept,
        ],
    );
    let state = example_contents();
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(before, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [~] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [ ] + after text 2⏎                                                         "
    "       23 this is some trailing text⏎                                           "
    "[×] baz                                                                      [-]"
    "        1 Some leading text 1⏎                                                  "
    "        2 Some leading text 2⏎                                                  "
    "  [×] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [×] + after text 2⏎                                                         "
    "###);
    insta::assert_display_snapshot!(after, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [~] Section 1/1                                                            [-]"
    "    [ ] - before text 1⏎                                                        "
    "    [ ] - before text 2⏎                                                        "
    "    [ ] + after text 1⏎                                                         "
    "    [×] + after text 2⏎                                                         "
    "       23 this is some trailing text⏎                                           "
    "[ ] baz                                                                      [-]"
    "        1 Some leading text 1⏎                                                  "
    "        2 Some leading text 2⏎                                                  "
    "  [ ] Section 1/1                                                            [-]"
    "    [ ] - before text 1⏎                                                        "
    "    [ ] - before text 2⏎                                                        "
    "    [ ] + after text 1⏎                                                         "
    "    [ ] + after text 2⏎                                                         "
    "###);
    Ok(())
}

#[test]
fn test_toggle_all_uniform() -> eyre::Result<()> {
    let initial = TestingScreenshot::default();
    let first_toggle = TestingScreenshot::default();
    let second_toggle = TestingScreenshot::default();
    let third_toggle = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        10,
        [
            Event::ExpandAll,
            initial.event(),
            Event::ToggleAllUniform,
            first_toggle.event(),
            Event::ToggleAllUniform,
            second_toggle.event(),
            Event::ToggleAllUniform,
            third_toggle.event(),
            Event::QuitAccept,
        ],
    );
    let state = example_contents();
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [~] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "###);
    insta::assert_display_snapshot!(first_toggle, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(×) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [×] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "###);
    insta::assert_display_snapshot!(second_toggle, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "( ) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [ ] Section 1/1                                                            [-]"
    "    [ ] - before text 1⏎                                                        "
    "    [ ] - before text 2⏎                                                        "
    "    [ ] + after text 1⏎                                                         "
    "###);
    insta::assert_display_snapshot!(third_toggle, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(×) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [×] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
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
            Event::ExpandAll,
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
    "[File] [Edit] [Select] [View]                                                                       "
    "(~) foo/bar                                                                                      (-)"
    "        ⋮                                                                                           "
    "       18 this is some text⏎                                                                        "
    "       19 this is some text⏎                                                                        "
    "       20 this is some text⏎                                                                        "
    "  [~] Section 1/1                                                                                [-]"
    "    [×] - before text 1⏎                                                                            "
    "    [×] - before text 2⏎                                                                            "
    "    [×] + after text 1⏎                                                                             "
    "    [ ] + after text 2⏎                                                                             "
    "       23 this is some trailing text⏎                                                               "
    "[×] baz                                                                                          [-]"
    "        1 Some leading text 1⏎                                                                      "
    "        2 Some leading text 2⏎                                                                      "
    "  [×] Section 1/1                                                                                [-]"
    "    [×] - before te┌Quit───────────────────────────────────────────────────────┐                    "
    "    [×] - before te│You have changes to 2 files. Are you sure you want to quit?│                    "
    "    [×] + after tex│                                                           │                    "
    "    [×] + after tex│                                                           │                    "
    "        5 this is s│                                                           │                    "
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
            Event::ExpandAll,
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
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/b┌Quit───────────────────────────────────────────────────────┐       (-)"
    "        ⋮│You have changes to 2 files. Are you sure you want to quit?│          "
    "       18└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "###);
    insta::assert_display_snapshot!(expect_c_does_nothing, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/b┌Quit───────────────────────────────────────────────────────┐       (-)"
    "        ⋮│You have changes to 2 files. Are you sure you want to quit?│          "
    "       18└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "###);
    insta::assert_display_snapshot!(expect_q_closes_quit_dialog, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "###);
    insta::assert_display_snapshot!(expect_ctrl_c_opens_quit_dialog, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/b┌Quit───────────────────────────────────────────────────────┐       (-)"
    "        ⋮│You have changes to 2 files. Are you sure you want to quit?│          "
    "       18└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
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
            Event::ExpandAll,
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
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/b┌Quit───────────────────────────────────────────────────────┐       (-)"
    "        ⋮│You have changes to 2 files. Are you sure you want to quit?│          "
    "       18└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "###);
    insta::assert_display_snapshot!(expect_left_focuses_go_back_button, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/b┌Quit───────────────────────────────────────────────────────┐       (-)"
    "        ⋮│You have changes to 2 files. Are you sure you want to quit?│          "
    "       18└───────────────────────────────────────────(Go Back)─[Quit]┘          "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "###);
    insta::assert_display_snapshot!(expect_left_again_does_not_wrap, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/b┌Quit───────────────────────────────────────────────────────┐       (-)"
    "        ⋮│You have changes to 2 files. Are you sure you want to quit?│          "
    "       18└───────────────────────────────────────────(Go Back)─[Quit]┘          "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "###);
    insta::assert_display_snapshot!(expect_back_button_closes_quit_dialog, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "###);
    insta::assert_display_snapshot!(expect_right_focuses_quit_button, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/b┌Quit───────────────────────────────────────────────────────┐       (-)"
    "        ⋮│You have changes to 2 files. Are you sure you want to quit?│          "
    "       18└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "###);
    insta::assert_display_snapshot!(expect_right_again_does_not_wrap, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/b┌Quit───────────────────────────────────────────────────────┐       (-)"
    "        ⋮│You have changes to 2 files. Are you sure you want to quit?│          "
    "       18└───────────────────────────────────────────[Go Back]─(Quit)┘          "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "###);
    insta::assert_display_snapshot!(expect_exited, @"<this screenshot was never assigned>");
    Ok(())
}

#[test]
fn test_enter_next() -> eyre::Result<()> {
    let state = RecordState {
        is_read_only: false,
        files: vec![
            File {
                old_path: None,
                path: Cow::Borrowed(Path::new("foo")),
                file_mode: None,
                sections: vec![Section::Changed {
                    lines: vec![
                        SectionChangedLine {
                            is_checked: false,
                            change_type: ChangeType::Added,
                            line: Cow::Borrowed("world\n"),
                        },
                        SectionChangedLine {
                            is_checked: false,
                            change_type: ChangeType::Removed,
                            line: Cow::Borrowed("hello\n"),
                        },
                    ],
                }],
            },
            File {
                old_path: None,
                path: Cow::Borrowed(Path::new("bar")),
                file_mode: None,
                sections: vec![Section::Changed {
                    lines: vec![
                        SectionChangedLine {
                            is_checked: false,
                            change_type: ChangeType::Added,
                            line: Cow::Borrowed("world\n"),
                        },
                        SectionChangedLine {
                            is_checked: false,
                            change_type: ChangeType::Removed,
                            line: Cow::Borrowed("hello\n"),
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
        7,
        [
            Event::ExpandAll,
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
    "[File] [Edit] [Select] [View]                                                   "
    "[×] foo                                                                      [-]"
    "    [×] - hello⏎                                                                "
    "( ) bar                                                                      (-)"
    "  [ ] Section 1/1                                                            [-]"
    "    [ ] + world⏎                                                                "
    "    [ ] - hello⏎                                                                "
    "###);
    insta::assert_display_snapshot!(second_file_selected, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[×] foo                                                                      [-]"
    "    [×] - hello⏎                                                                "
    "(×) bar                                                                      (-)"
    "  [×] Section 1/1                                                            [-]"
    "    [×] + world⏎                                                                "
    "    [×] - hello⏎                                                                "
    "###);
    Ok(())
}

#[test]
fn test_file_mode_change() -> eyre::Result<()> {
    let state = RecordState {
        is_read_only: false,
        files: vec![
            File {
                old_path: None,
                path: Cow::Borrowed(Path::new("foo")),
                file_mode: None,
                sections: vec![],
            },
            File {
                old_path: None,
                path: Cow::Borrowed(Path::new("bar")),
                file_mode: None,
                sections: vec![Section::FileMode {
                    is_checked: false,
                    before: FileMode(0o100644),
                    after: FileMode(0o100755),
                }],
            },
            File {
                old_path: None,
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
            Event::ExpandAll,
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
        is_read_only: false,
        files: [
            File {
                old_path: None,
                path: "foo",
                file_mode: None,
                sections: [],
            },
            File {
                old_path: None,
                path: "bar",
                file_mode: None,
                sections: [
                    FileMode {
                        is_checked: true,
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
                old_path: None,
                path: "qux",
                file_mode: None,
                sections: [],
            },
        ],
    }
    "###);
    insta::assert_display_snapshot!(before_toggle, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "( ) foo                                                                      (-)"
    "[ ] bar                                                                      [-]"
    "  [ ] File mode changed from 100644 to 100755                                   "
    "[ ] qux                                                                      [-]"
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_toggle, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[ ] foo                                                                      [-]"
    "[×] bar                                                                      [-]"
    "  (×) File mode changed from 100644 to 100755                                   "
    "[ ] qux                                                                      [-]"
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(expect_no_crash, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[ ] foo                                                                      [-]"
    "[×] bar                                                                      [-]"
    "  [×] File mode changed from 100644 to 100755                                   "
    "( ) qux                                                                      (-)"
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
        is_read_only: false,
        files: vec![File {
            old_path: None,
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
                        is_checked: false,
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
                        is_checked: false,
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
    let event_source = EventSource::testing(
        80,
        24,
        [Event::ExpandAll, screenshot.event(), Event::QuitAccept],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;
    insta::assert_display_snapshot!(screenshot, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "( ) foo                                                                      (-)"
    "        ⋮                                                                       "
    "        4 start line 4/6⏎                                                       "
    "        5 start line 5/6⏎                                                       "
    "        6 start line 6/6⏎                                                       "
    "  [ ] Section 1/2                                                            [-]"
    "    [ ] + changed⏎                                                              "
    "        7 middle line 1/7⏎                                                      "
    "        8 middle line 2/7⏎                                                      "
    "        9 middle line 3/7⏎                                                      "
    "        ⋮                                                                       "
    "       11 middle line 5/7⏎                                                      "
    "       12 middle line 6/7⏎                                                      "
    "       13 middle line 7/7⏎                                                      "
    "  [ ] Section 2/2                                                            [-]"
    "    [ ] + changed⏎                                                              "
    "       14 end line 1/6⏎                                                         "
    "       15 end line 2/6⏎                                                         "
    "       16 end line 3/6⏎                                                         "
    "        ⋮                                                                       "
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
        is_read_only: false,
        files: vec![File {
            old_path: None,
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
                        is_checked: false,
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
                        is_checked: false,
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
    let event_source = EventSource::testing(
        80,
        20,
        [Event::ExpandAll, screenshot.event(), Event::QuitAccept],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;
    insta::assert_display_snapshot!(screenshot, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "( ) foo                                                                      (-)"
    "        1 start line 1/2⏎                                                       "
    "        2 start line 2/2⏎                                                       "
    "  [ ] Section 1/2                                                            [-]"
    "    [ ] + changed⏎                                                              "
    "        3 middle line 1/6⏎                                                      "
    "        4 middle line 2/6⏎                                                      "
    "        5 middle line 3/6⏎                                                      "
    "        6 middle line 4/6⏎                                                      "
    "        7 middle line 5/6⏎                                                      "
    "        8 middle line 6/6⏎                                                      "
    "  [ ] Section 2/2                                                            [-]"
    "    [ ] + changed⏎                                                              "
    "        9 end line 1/2⏎                                                         "
    "       10 end line 2/2⏎                                                         "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);

    Ok(())
}

#[test]
fn test_record_binary_file() -> eyre::Result<()> {
    let state = RecordState {
        is_read_only: false,
        files: vec![File {
            old_path: None,
            path: Cow::Borrowed(Path::new("foo")),
            file_mode: None,
            sections: vec![Section::Binary {
                is_checked: false,
                old_description: Some(Cow::Owned(make_binary_description("abc123", 123))),
                new_description: Some(Cow::Owned(make_binary_description("def456", 456))),
            }],
        }],
    };

    let initial = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        6,
        [
            Event::ExpandAll,
            initial.event(),
            Event::ToggleItem,
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    let state = recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "( ) foo                                                                      (-)"
    "  [ ] (binary contents: abc123 (123 bytes) -> def456 (456 bytes))               "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);

    assert_debug_snapshot!(state, @r###"
    RecordState {
        is_read_only: false,
        files: [
            File {
                old_path: None,
                path: "foo",
                file_mode: None,
                sections: [
                    Binary {
                        is_checked: true,
                        old_description: Some(
                            "abc123 (123 bytes)",
                        ),
                        new_description: Some(
                            "def456 (456 bytes)",
                        ),
                    },
                ],
            },
        ],
    }
    "###);

    let (selected, unselected) = state.files[0].get_selected_contents();
    assert_debug_snapshot!(selected, @r###"
    Binary {
        old_description: Some(
            "abc123 (123 bytes)",
        ),
        new_description: Some(
            "def456 (456 bytes)",
        ),
    }
    "###);
    assert_debug_snapshot!(unselected, @"Unchanged");

    Ok(())
}

#[test]
fn test_record_binary_file_noop() -> eyre::Result<()> {
    let state = RecordState {
        is_read_only: false,
        files: vec![File {
            old_path: None,
            path: Cow::Borrowed(Path::new("foo")),
            file_mode: None,
            sections: vec![Section::Binary {
                is_checked: false,
                old_description: Some(Cow::Owned(make_binary_description("abc123", 123))),
                new_description: Some(Cow::Owned(make_binary_description("def456", 456))),
            }],
        }],
    };

    let initial = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        6,
        [Event::ExpandAll, initial.event(), Event::QuitAccept],
    );
    let recorder = Recorder::new(state, event_source);
    let state = recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "( ) foo                                                                      (-)"
    "  [ ] (binary contents: abc123 (123 bytes) -> def456 (456 bytes))               "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);

    assert_debug_snapshot!(state, @r###"
    RecordState {
        is_read_only: false,
        files: [
            File {
                old_path: None,
                path: "foo",
                file_mode: None,
                sections: [
                    Binary {
                        is_checked: false,
                        old_description: Some(
                            "abc123 (123 bytes)",
                        ),
                        new_description: Some(
                            "def456 (456 bytes)",
                        ),
                    },
                ],
            },
        ],
    }
    "###);

    let (selected, unselected) = state.files[0].get_selected_contents();
    assert_debug_snapshot!(selected, @"Unchanged");
    assert_debug_snapshot!(unselected, @r###"
    Binary {
        old_description: Some(
            "abc123 (123 bytes)",
        ),
        new_description: Some(
            "def456 (456 bytes)",
        ),
    }
    "###);

    Ok(())
}

#[test]
fn test_state_binary_selected_contents() -> eyre::Result<()> {
    let test = |is_checked, binary| {
        let file = File {
            old_path: None,
            path: Cow::Borrowed(Path::new("foo")),
            file_mode: None,
            sections: vec![
                Section::Changed {
                    lines: vec![SectionChangedLine {
                        is_checked,
                        change_type: ChangeType::Removed,
                        line: Cow::Borrowed("foo\n"),
                    }],
                },
                Section::Binary {
                    is_checked: binary,
                    old_description: Some(Cow::Owned(make_binary_description("abc123", 123))),
                    new_description: Some(Cow::Owned(make_binary_description("def456", 456))),
                },
            ],
        };
        let selection = file.get_selected_contents();
        format!("{selection:?}")
    };

    assert_snapshot!(test(false, false), @r###"(Present { contents: "foo\n" }, Binary { old_description: Some("abc123 (123 bytes)"), new_description: Some("def456 (456 bytes)") })"###);
    assert_snapshot!(test(true, false), @r###"(Unchanged, Binary { old_description: Some("abc123 (123 bytes)"), new_description: Some("def456 (456 bytes)") })"###);

    // NB: The result for this situation, where we've selected both a text and
    // binary segment for inclusion, is arbitrary. The caller should avoid
    // generating both kinds of sections in the same file (or we should improve
    // the UI to never allow selecting both).
    assert_snapshot!(test(false, true), @r###"(Binary { old_description: Some("abc123 (123 bytes)"), new_description: Some("def456 (456 bytes)") }, Unchanged)"###);

    assert_snapshot!(test(true, true), @r###"(Binary { old_description: Some("abc123 (123 bytes)"), new_description: Some("def456 (456 bytes)") }, Present { contents: "foo\n" })"###);

    Ok(())
}

#[test]
fn test_mouse_support() -> eyre::Result<()> {
    let state = example_contents();

    let initial = TestingScreenshot::default();
    let first_click = TestingScreenshot::default();
    let click_scrolled_item = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        7,
        [
            Event::ExpandAll,
            initial.event(),
            Event::Click { row: 6, column: 8 },
            Event::EnsureSelectionInViewport,
            first_click.event(),
            Event::Click { row: 6, column: 8 },
            Event::EnsureSelectionInViewport,
            click_scrolled_item.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [~] Section 1/1                                                            [-]"
    "###);
    insta::assert_display_snapshot!(first_click, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[~] foo/bar                                                                  [-]"
    "  (~) Section 1/1                                                            (-)"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [ ] + after text 2⏎                                                         "
    "###);
    insta::assert_display_snapshot!(click_scrolled_item, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[~] foo/bar                                                                  [-]"
    "  [~] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    ( ) + after text 2⏎                                                         "
    "###);

    Ok(())
}
#[test]
fn test_mouse_click_checkbox() -> eyre::Result<()> {
    let state = RecordState {
        is_read_only: false,
        files: vec![
            File {
                old_path: None,
                path: Cow::Borrowed(Path::new("foo")),
                file_mode: None,
                sections: vec![],
            },
            File {
                old_path: None,
                path: Cow::Borrowed(Path::new("bar")),
                file_mode: None,
                sections: vec![Section::FileMode {
                    is_checked: false,
                    before: FileMode::absent(),
                    after: FileMode(0o100644),
                }],
            },
        ],
    };

    let initial = TestingScreenshot::default();
    let click_unselected_checkbox = TestingScreenshot::default();
    let click_selected_checkbox = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        4,
        [
            Event::ExpandAll,
            initial.event(),
            Event::Click { row: 2, column: 1 },
            click_unselected_checkbox.event(),
            Event::Click { row: 2, column: 1 },
            click_selected_checkbox.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "( ) foo                                                                      (-)"
    "[ ] bar                                                                      [-]"
    "  [ ] File mode changed from 0 to 100644                                        "
    "###);
    insta::assert_display_snapshot!(click_unselected_checkbox, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[ ] foo                                                                      [-]"
    "( ) bar                                                                      (-)"
    "  [ ] File mode changed from 0 to 100644                                        "
    "###);
    insta::assert_display_snapshot!(click_selected_checkbox, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[ ] foo                                                                      [-]"
    "(×) bar                                                                      (-)"
    "  [×] File mode changed from 0 to 100644                                        "
    "###);

    Ok(())
}

#[test]
fn test_mouse_click_wide_line() -> eyre::Result<()> {
    let state = RecordState {
        is_read_only: false,
        files: vec![File {
            old_path: None,
            path: Cow::Borrowed(Path::new("foo")),
            file_mode: None,
            sections: vec![
                Section::FileMode {
                    is_checked: false,
                    before: FileMode::absent(),
                    after: FileMode(0o100644),
                },
                Section::Changed {
                    lines: vec![SectionChangedLine {
                        is_checked: false,
                        change_type: ChangeType::Removed,
                        line: Cow::Borrowed("foo\n"),
                    }],
                },
            ],
        }],
    };

    let initial = TestingScreenshot::default();
    let click_line = TestingScreenshot::default();
    let click_line_section = TestingScreenshot::default();
    let click_file_mode_section = TestingScreenshot::default();
    let click_file = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        5,
        [
            Event::ExpandAll,
            initial.event(),
            Event::Click { row: 4, column: 50 },
            click_line.event(),
            Event::Click { row: 3, column: 50 },
            click_line_section.event(),
            Event::Click { row: 2, column: 50 },
            click_file_mode_section.event(),
            Event::Click { row: 1, column: 50 },
            click_file.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "( ) foo                                                                      (-)"
    "  [ ] File mode changed from 0 to 100644                                        "
    "  [ ] Section 2/2                                                            [-]"
    "    [ ] - foo⏎                                                                  "
    "###);
    insta::assert_display_snapshot!(click_line, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[ ] foo                                                                      [-]"
    "  [ ] File mode changed from 0 to 100644                                        "
    "  [ ] Section 2/2                                                            [-]"
    "    ( ) - foo⏎                                                                  "
    "###);
    insta::assert_display_snapshot!(click_line_section, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[ ] foo                                                                      [-]"
    "  [ ] File mode changed from 0 to 100644                                        "
    "  ( ) Section 2/2                                                            (-)"
    "    [ ] - foo⏎                                                                  "
    "###);
    insta::assert_display_snapshot!(click_file_mode_section, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[ ] foo                                                                      [-]"
    "  ( ) File mode changed from 0 to 100644                                        "
    "  [ ] Section 2/2                                                            [-]"
    "    [ ] - foo⏎                                                                  "
    "###);
    insta::assert_display_snapshot!(click_file, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "( ) foo                                                                      (-)"
    "  [ ] File mode changed from 0 to 100644                                        "
    "  [ ] Section 2/2                                                            [-]"
    "    [ ] - foo⏎                                                                  "
    "###);

    Ok(())
}

#[test]
fn test_mouse_click_dialog_buttons() -> eyre::Result<()> {
    let state = RecordState {
        is_read_only: false,
        files: vec![File {
            old_path: None,
            path: Cow::Borrowed(Path::new("foo")),
            file_mode: None,
            sections: vec![Section::Changed {
                lines: vec![SectionChangedLine {
                    is_checked: true,
                    change_type: ChangeType::Removed,
                    line: Cow::Borrowed("foo\n"),
                }],
            }],
        }],
    };

    let click_nothing = TestingScreenshot::default();
    let click_go_back = TestingScreenshot::default();
    let events = [
        Event::ExpandAll,
        Event::QuitCancel,
        Event::Click { row: 3, column: 55 },
        click_nothing.event(),
        Event::QuitCancel,
        Event::Click { row: 3, column: 65 },
        click_go_back.event(),
    ];
    let event_source = EventSource::testing(80, 6, events);
    let recorder = Recorder::new(state, event_source);
    let result = recorder.run();
    insta::assert_debug_snapshot!(result, @r###"
    Err(
        Cancelled,
    )
    "###);

    insta::assert_display_snapshot!(click_nothing, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(×) foo                                                                      (-)"
    "  [×] Section 1/1                                                            [-]"
    "    [×] - foo⏎                                                                  "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(click_go_back, @"<this screenshot was never assigned>");

    Ok(())
}

#[test]
fn test_render_old_path() -> eyre::Result<()> {
    let state = RecordState {
        is_read_only: false,
        files: vec![File {
            old_path: Some(Cow::Borrowed(Path::new("foo"))),
            path: Cow::Borrowed(Path::new("bar")),
            file_mode: None,
            sections: vec![],
        }],
    };
    let screenshot = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        6,
        [Event::ExpandAll, screenshot.event(), Event::QuitAccept],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(screenshot, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "( ) foo => bar                                                               (-)"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);

    Ok(())
}

#[test]
fn test_expand() -> eyre::Result<()> {
    let state = example_contents();
    let initial = TestingScreenshot::default();
    let after_expand = TestingScreenshot::default();
    let after_collapse = TestingScreenshot::default();
    let after_expand_mouse = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        7,
        [
            initial.event(),
            Event::ExpandItem,
            after_expand.event(),
            Event::ExpandItem,
            after_collapse.event(),
            Event::Click { row: 2, column: 78 },
            Event::Click { row: 2, column: 78 },
            after_expand_mouse.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (+)"
    "[×] baz                                                                      [+]"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_expand, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [~] Section 1/1                                                            [-]"
    "###);
    insta::assert_display_snapshot!(after_collapse, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (+)"
    "[×] baz                                                                      [+]"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_expand_mouse, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(×) baz                                                                      (-)"
    "        1 Some leading text 1⏎                                                  "
    "        2 Some leading text 2⏎                                                  "
    "  [×] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "###);

    Ok(())
}

#[test]
fn test_expand_line_noop() -> eyre::Result<()> {
    let state = example_contents();
    let after_select = TestingScreenshot::default();
    let after_expand_noop = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        7,
        [
            Event::ExpandAll,
            Event::FocusNext,
            Event::FocusNext,
            after_select.event(),
            Event::ExpandItem,
            after_expand_noop.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(after_select, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[~] foo/bar                                                                  [-]"
    "  [~] Section 1/1                                                            [-]"
    "    (×) - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [ ] + after text 2⏎                                                         "
    "###);
    insta::assert_display_snapshot!(after_expand_noop, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[~] foo/bar                                                                  [-]"
    "  [~] Section 1/1                                                            [-]"
    "    (×) - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [ ] + after text 2⏎                                                         "
    "###);

    Ok(())
}

#[test]
fn test_expand_scroll_into_view() -> eyre::Result<()> {
    let state = example_contents();
    let before_expand = TestingScreenshot::default();
    let after_expand = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        7,
        [
            Event::FocusNext,
            before_expand.event(),
            Event::ExpandAll,
            after_expand.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(before_expand, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[~] foo/bar                                                                  [+]"
    "(×) baz                                                                      (+)"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_expand, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(×) baz                                                                      (-)"
    "        1 Some leading text 1⏎                                                  "
    "        2 Some leading text 2⏎                                                  "
    "  [×] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "###);

    Ok(())
}

#[test]
fn test_collapse_select_ancestor() -> eyre::Result<()> {
    let state = example_contents();
    let before_collapse = TestingScreenshot::default();
    let after_collapse = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        7,
        [
            Event::ExpandAll,
            Event::FocusNext,
            before_collapse.event(),
            Event::ExpandAll,
            after_collapse.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(before_collapse, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[~] foo/bar                                                                  [-]"
    "  (~) Section 1/1                                                            (-)"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [ ] + after text 2⏎                                                         "
    "###);
    insta::assert_display_snapshot!(after_collapse, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (+)"
    "[×] baz                                                                      [+]"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);

    Ok(())
}

#[test]
fn test_focus_inner() -> eyre::Result<()> {
    let state = example_contents();
    let initial = TestingScreenshot::default();
    let inner1 = TestingScreenshot::default();
    let inner2 = TestingScreenshot::default();
    let inner3 = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        7,
        [
            initial.event(),
            Event::FocusInner,
            inner1.event(),
            Event::FocusInner,
            inner2.event(),
            Event::FocusInner,
            inner3.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (+)"
    "[×] baz                                                                      [+]"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(inner1, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[~] foo/bar                                                                  [-]"
    "  (~) Section 1/1                                                            (-)"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [ ] + after text 2⏎                                                         "
    "###);
    insta::assert_display_snapshot!(inner2, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[~] foo/bar                                                                  [-]"
    "  [~] Section 1/1                                                            [-]"
    "    (×) - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [ ] + after text 2⏎                                                         "
    "###);
    insta::assert_display_snapshot!(inner3, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[~] foo/bar                                                                  [-]"
    "  [~] Section 1/1                                                            [-]"
    "    (×) - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [ ] + after text 2⏎                                                         "
    "###);

    Ok(())
}

#[test]
fn test_focus_outer() -> eyre::Result<()> {
    let state = example_contents();
    let initial = TestingScreenshot::default();
    let outer1 = TestingScreenshot::default();
    let outer2 = TestingScreenshot::default();
    let outer3 = TestingScreenshot::default();
    let outer4 = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        7,
        [
            Event::FocusNext,
            Event::ExpandItem,
            Event::FocusNext,
            Event::FocusNext,
            Event::FocusNext,
            initial.event(),
            Event::FocusOuter,
            outer1.event(),
            Event::FocusOuter,
            outer2.event(),
            Event::FocusOuter,
            outer3.event(),
            Event::FocusOuter,
            outer4.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[×] baz                                                                      [-]"
    "  [×] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    (×) - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [×] + after text 2⏎                                                         "
    "###);
    insta::assert_display_snapshot!(outer1, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[×] baz                                                                      [-]"
    "  (×) Section 1/1                                                            (-)"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [×] + after text 2⏎                                                         "
    "###);
    insta::assert_display_snapshot!(outer2, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(×) baz                                                                      (-)"
    "        1 Some leading text 1⏎                                                  "
    "        2 Some leading text 2⏎                                                  "
    "  [×] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "###);
    insta::assert_display_snapshot!(outer3, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(×) baz                                                                      (+)"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(outer4, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(×) baz                                                                      (+)"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);

    Ok(())
}

#[test]
fn test_sticky_header_scroll() -> eyre::Result<()> {
    let state = example_contents();
    let initial = TestingScreenshot::default();
    let scroll1 = TestingScreenshot::default();
    let scroll2 = TestingScreenshot::default();
    let scroll3 = TestingScreenshot::default();
    let scroll4 = TestingScreenshot::default();
    let scroll5 = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        7,
        [
            Event::ExpandAll,
            initial.event(),
            Event::ScrollDown,
            scroll1.event(),
            Event::ScrollDown,
            scroll2.event(),
            Event::ScrollDown,
            scroll3.event(),
            Event::ScrollDown,
            scroll4.event(),
            Event::ScrollDown,
            scroll5.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [~] Section 1/1                                                            [-]"
    "###);
    insta::assert_display_snapshot!(scroll1, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [~] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "###);
    insta::assert_display_snapshot!(scroll2, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [~] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "###);
    insta::assert_display_snapshot!(scroll3, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "       20 this is some text⏎                                                    "
    "  [~] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "###);
    insta::assert_display_snapshot!(scroll4, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "  [~] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [ ] + after text 2⏎                                                         "
    "###);
    insta::assert_display_snapshot!(scroll5, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [ ] + after text 2⏎                                                         "
    "       23 this is some trailing text⏎                                           "
    "###);

    Ok(())
}

#[test]
fn test_sticky_header_click_expand() -> eyre::Result<()> {
    let state = example_contents();
    let initial = TestingScreenshot::default();
    let after_scroll = TestingScreenshot::default();
    let after_click1 = TestingScreenshot::default();
    let after_click2 = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        7,
        [
            initial.event(),
            Event::FocusNext,
            Event::ExpandItem,
            Event::FocusNext,
            after_scroll.event(),
            Event::Click { row: 1, column: 70 },
            after_click1.event(),
            Event::Click { row: 1, column: 78 },
            after_click2.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (+)"
    "[×] baz                                                                      [+]"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_scroll, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[×] baz                                                                      [-]"
    "  (×) Section 1/1                                                            (-)"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [×] + after text 2⏎                                                         "
    "###);
    insta::assert_display_snapshot!(after_click1, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(×) baz                                                                      (-)"
    "  [×] Section 1/1                                                            [-]"
    "    [×] - before text 1⏎                                                        "
    "    [×] - before text 2⏎                                                        "
    "    [×] + after text 1⏎                                                         "
    "    [×] + after text 2⏎                                                         "
    "###);
    insta::assert_display_snapshot!(after_click2, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(×) baz                                                                      (+)"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);

    Ok(())
}

#[test]
fn test_scroll_click_no_jump() -> eyre::Result<()> {
    let state = example_contents();
    let initial = TestingScreenshot::default();
    let after_click = TestingScreenshot::default();
    let event_source = EventSource::testing(
        80,
        7,
        [
            Event::ExpandAll,
            initial.event(),
            Event::Click { row: 5, column: 5 },
            after_click.event(),
            Event::QuitAccept,
        ],
    );
    let recorder = Recorder::new(state, event_source);
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [~] Section 1/1                                                            [-]"
    "###);
    insta::assert_display_snapshot!(after_click, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[~] foo/bar                                                                  [-]"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  [~] Section 1/1                                                            [-]"
    "###);

    Ok(())
}

#[test]
fn test_menu_bar_scroll_into_view() -> eyre::Result<()> {
    let state = example_contents();
    let initial = TestingScreenshot::default();
    let after_scroll1 = TestingScreenshot::default();
    let after_scroll2 = TestingScreenshot::default();
    let recorder = Recorder::new(
        state,
        EventSource::testing(
            80,
            6,
            [
                initial.event(),
                Event::ScrollDown,
                after_scroll1.event(),
                Event::ScrollDown,
                after_scroll2.event(),
                Event::QuitAccept,
            ],
        ),
    );
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (+)"
    "[×] baz                                                                      [+]"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_scroll1, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[×] baz                                                                      [+]"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_scroll2, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[×] baz                                                                      [+]"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);

    Ok(())
}

#[test]
fn test_expand_menu() -> eyre::Result<()> {
    let state = example_contents();
    let initial = TestingScreenshot::default();
    let after_click = TestingScreenshot::default();
    let after_click_different = TestingScreenshot::default();
    let after_click_same = TestingScreenshot::default();
    let after_click_menu_bar = TestingScreenshot::default();
    let recorder = Recorder::new(
        state,
        EventSource::testing(
            80,
            6,
            [
                initial.event(),
                Event::Click { row: 0, column: 8 },
                after_click.event(),
                Event::Click { row: 0, column: 0 },
                after_click_different.event(),
                Event::Click { row: 0, column: 0 },
                after_click_same.event(),
                Event::Click { row: 0, column: 79 },
                after_click_menu_bar.event(),
                Event::QuitAccept,
            ],
        ),
    );
    recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (+)"
    "[×] baz                                                                      [+]"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_click, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo[Toggle current (space)]                                              (+)"
    "[×] baz[Toggle current and advance (enter)]                                  [+]"
    "       [Invert all items (a)]                                                   "
    "       [Invert all items uniformly (A)]                                         "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_click_different, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "[Confirm (c)]                                                                (+)"
    "[Quit (q)]                                                                   [+]"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_click_same, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (+)"
    "[×] baz                                                                      [+]"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_click_menu_bar, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (+)"
    "[×] baz                                                                      [+]"
    "                                                                                "
    "                                                                                "
    "                                                                                "
    "###);

    Ok(())
}

#[test]
fn test_read_only() -> eyre::Result<()> {
    let state = RecordState {
        is_read_only: true,
        ..example_contents()
    };
    let initial = TestingScreenshot::default();
    let after_toggle_ignored = TestingScreenshot::default();
    let recorder = Recorder::new(
        state,
        EventSource::testing(
            80,
            23,
            [
                Event::ExpandAll,
                initial.event(),
                Event::ToggleAll,
                after_toggle_ignored.event(),
                Event::QuitAccept,
            ],
        ),
    );
    let state = recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "〈~〉 foo/bar                                                                (-)" Hidden by multi-width symbols: [(1, " "), (4, " ")]
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  〈~〉 Section 1/1                                                          [-]" Hidden by multi-width symbols: [(3, " "), (6, " ")]
    "    〈×〉 - before text 1⏎                                                      " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "    〈×〉 - before text 2⏎                                                      " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "    〈×〉 + after text 1⏎                                                       " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "    〈 〉 + after text 2⏎                                                       " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "       23 this is some trailing text⏎                                           "
    "〈×〉 baz                                                                    [-]" Hidden by multi-width symbols: [(1, " "), (4, " ")]
    "        1 Some leading text 1⏎                                                  "
    "        2 Some leading text 2⏎                                                  "
    "  〈×〉 Section 1/1                                                          [-]" Hidden by multi-width symbols: [(3, " "), (6, " ")]
    "    〈×〉 - before text 1⏎                                                      " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "    〈×〉 - before text 2⏎                                                      " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "    〈×〉 + after text 1⏎                                                       " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "    〈×〉 + after text 2⏎                                                       " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "        5 this is some trailing text⏎                                           "
    "                                                                                "
    "                                                                                "
    "###);
    insta::assert_display_snapshot!(after_toggle_ignored, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "〈~〉 foo/bar                                                                (-)" Hidden by multi-width symbols: [(1, " "), (4, " ")]
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "  〈~〉 Section 1/1                                                          [-]" Hidden by multi-width symbols: [(3, " "), (6, " ")]
    "    〈 〉 - before text 1⏎                                                      " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "    〈 〉 - before text 2⏎                                                      " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "    〈 〉 + after text 1⏎                                                       " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "    〈×〉 + after text 2⏎                                                       " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "       23 this is some trailing text⏎                                           "
    "〈 〉 baz                                                                    [-]" Hidden by multi-width symbols: [(1, " "), (4, " ")]
    "        1 Some leading text 1⏎                                                  "
    "        2 Some leading text 2⏎                                                  "
    "  〈 〉 Section 1/1                                                          [-]" Hidden by multi-width symbols: [(3, " "), (6, " ")]
    "    〈 〉 - before text 1⏎                                                      " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "    〈 〉 - before text 2⏎                                                      " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "    〈 〉 + after text 1⏎                                                       " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "    〈 〉 + after text 2⏎                                                       " Hidden by multi-width symbols: [(5, " "), (8, " ")]
    "        5 this is some trailing text⏎                                           "
    "                                                                                "
    "                                                                                "
    "###);

    insta::assert_debug_snapshot!(state, @r###"
    RecordState {
        is_read_only: true,
        files: [
            File {
                old_path: None,
                path: "foo/bar",
                file_mode: None,
                sections: [
                    Unchanged {
                        lines: [
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                        ],
                    },
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Removed,
                                line: "before text 1\n",
                            },
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Removed,
                                line: "before text 2\n",
                            },
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "after text 1\n",
                            },
                            SectionChangedLine {
                                is_checked: true,
                                change_type: Added,
                                line: "after text 2\n",
                            },
                        ],
                    },
                    Unchanged {
                        lines: [
                            "this is some trailing text\n",
                        ],
                    },
                ],
            },
            File {
                old_path: None,
                path: "baz",
                file_mode: None,
                sections: [
                    Unchanged {
                        lines: [
                            "Some leading text 1\n",
                            "Some leading text 2\n",
                        ],
                    },
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Removed,
                                line: "before text 1\n",
                            },
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Removed,
                                line: "before text 2\n",
                            },
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "after text 1\n",
                            },
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "after text 2\n",
                            },
                        ],
                    },
                    Unchanged {
                        lines: [
                            "this is some trailing text\n",
                        ],
                    },
                ],
            },
        ],
    }
    "###);

    Ok(())
}

#[test]
fn test_toggle_unchanged_line() -> eyre::Result<()> {
    let state = example_contents();
    let initial = TestingScreenshot::default();
    let after_toggle = TestingScreenshot::default();
    let recorder = Recorder::new(
        state,
        EventSource::testing(
            80,
            6,
            [
                Event::ExpandAll,
                initial.event(),
                Event::Click { row: 4, column: 10 },
                Event::ToggleItem, // should not crash
                after_toggle.event(),
                Event::QuitAccept,
            ],
        ),
    );
    let state = recorder.run()?;

    insta::assert_display_snapshot!(initial, @r###"
    "[File] [Edit] [Select] [View]                                                   "
    "(~) foo/bar                                                                  (-)"
    "        ⋮                                                                       "
    "       18 this is some text⏎                                                    "
    "       19 this is some text⏎                                                    "
    "       20 this is some text⏎                                                    "
    "###);

    insta::assert_debug_snapshot!(state, @r###"
    RecordState {
        is_read_only: false,
        files: [
            File {
                old_path: None,
                path: "foo/bar",
                file_mode: None,
                sections: [
                    Unchanged {
                        lines: [
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                            "this is some text\n",
                        ],
                    },
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: true,
                                change_type: Removed,
                                line: "before text 1\n",
                            },
                            SectionChangedLine {
                                is_checked: true,
                                change_type: Removed,
                                line: "before text 2\n",
                            },
                            SectionChangedLine {
                                is_checked: true,
                                change_type: Added,
                                line: "after text 1\n",
                            },
                            SectionChangedLine {
                                is_checked: false,
                                change_type: Added,
                                line: "after text 2\n",
                            },
                        ],
                    },
                    Unchanged {
                        lines: [
                            "this is some trailing text\n",
                        ],
                    },
                ],
            },
            File {
                old_path: None,
                path: "baz",
                file_mode: None,
                sections: [
                    Unchanged {
                        lines: [
                            "Some leading text 1\n",
                            "Some leading text 2\n",
                        ],
                    },
                    Changed {
                        lines: [
                            SectionChangedLine {
                                is_checked: true,
                                change_type: Removed,
                                line: "before text 1\n",
                            },
                            SectionChangedLine {
                                is_checked: true,
                                change_type: Removed,
                                line: "before text 2\n",
                            },
                            SectionChangedLine {
                                is_checked: true,
                                change_type: Added,
                                line: "after text 1\n",
                            },
                            SectionChangedLine {
                                is_checked: true,
                                change_type: Added,
                                line: "after text 2\n",
                            },
                        ],
                    },
                    Unchanged {
                        lines: [
                            "this is some trailing text\n",
                        ],
                    },
                ],
            },
        ],
    }
    "###);

    Ok(())
}
