use std::borrow::Cow;
use std::cmp::min;
use std::path::Path;
use std::sync::mpsc::Sender;

use cursive::event::Event;
use cursive::theme::{BaseColor, Effect};
use cursive::traits::{Nameable, Resizable};
use cursive::utils::markup::StyledString;
use cursive::views::{Checkbox, Dialog, HideableView, LinearLayout, ScrollView, TextView};
use cursive::{CursiveRunnable, CursiveRunner, View};
use tracing::error;

use crate::cursive_utils::{EventDrivenCursiveApp, EventDrivenCursiveAppExt};
use crate::tristate::{Tristate, TristateBox};
use crate::views::ListView2;
use crate::{FileState, RecordError, RecordState, Section, SectionChangedLine};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SectionChangedLineType {
    Before,
    After,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileKey {
    file_num: usize,
}

impl FileKey {
    fn view_id(&self) -> String {
        let Self { file_num } = self;
        format!("FileKey({})", file_num)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SectionKey {
    file_num: usize,
    section_num: usize,
}

impl SectionKey {
    fn view_id(&self) -> String {
        let Self {
            file_num,
            section_num,
        } = self;
        format!("HunkKey({},{})", *file_num, section_num)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SectionLineKey {
    file_num: usize,
    section_num: usize,
    section_type: SectionChangedLineType,
    section_line_num: usize,
}

impl SectionLineKey {
    fn view_id(&self) -> String {
        let Self {
            file_num,
            section_num,
            section_type,
            section_line_num,
        } = self;
        format!(
            "HunkLineKey({},{},{},{})",
            file_num,
            section_num,
            match section_type {
                SectionChangedLineType::Before => "B",
                SectionChangedLineType::After => "A",
            },
            section_line_num
        )
    }
}

/// UI component to record the user's changes.
pub struct Recorder<'a> {
    did_user_confirm_exit: bool,
    state: RecordState<'a>,
}

impl<'a> Recorder<'a> {
    /// Constructor.
    pub fn new(state: RecordState<'a>) -> Self {
        Self {
            did_user_confirm_exit: false,
            state,
        }
    }

    /// Run the terminal user interface and have the user interactively select
    /// changes.
    pub fn run(self, siv: CursiveRunner<CursiveRunnable>) -> Result<RecordState<'a>, RecordError> {
        EventDrivenCursiveAppExt::run(self, siv)
    }

    fn make_main_view(&self, main_tx: Sender<Message>) -> impl View {
        let mut view = ListView2::new();

        let RecordState { file_states } = &self.state;

        let global_num_changed_sections: usize = file_states
            .iter()
            .map(|(_path, file_state)| file_state.count_changed_sections())
            .sum();
        let mut global_changed_section_num = 0;

        for (file_num, (path, file_state)) in file_states.iter().enumerate() {
            let file_key = FileKey { file_num };
            view.add_child(
                "",
                self.make_file_view(
                    main_tx.clone(),
                    path,
                    file_key,
                    file_state,
                    &mut global_changed_section_num,
                    global_num_changed_sections,
                ),
            );
            if file_num + 1 < file_states.len() {
                // Render a spacer line. Note that an empty string won't render an
                // empty line.
                view.add_child("", TextView::new(" "));
            }
        }

        view
    }

    fn make_file_view(
        &self,
        main_tx: Sender<Message>,
        path: &Path,
        file_key: FileKey,
        file_state: &FileState,
        global_changed_section_num: &mut usize,
        global_num_changed_sections: usize,
    ) -> impl View {
        let FileKey { file_num } = file_key;

        let mut file_view = ListView2::new();
        let mut line_num: usize = 1;
        let local_num_changed_sections = file_state.count_changed_sections();
        let mut local_changed_section_num = 0;

        let file_header_view = LinearLayout::horizontal()
            .child(
                TristateBox::new()
                    .with_state({
                        all_are_same_value(iter_file_selections(file_state)).into_tristate()
                    })
                    .on_change({
                        let main_tx = main_tx.clone();
                        move |_, new_value| {
                            if main_tx
                                .send(Message::ToggleFile(file_key, new_value))
                                .is_err()
                            {
                                // Do nothing.
                            }
                        }
                    })
                    .with_name(file_key.view_id()),
            )
            .child(TextView::new({
                let mut s = StyledString::new();
                s.append_plain(" ");
                s.append_styled(path.to_string_lossy(), Effect::Bold);
                s
            }));
        file_view.add_child("", file_header_view);

        let FileState {
            file_mode: _,
            sections,
        } = file_state;
        for (section_num, section) in sections.iter().enumerate() {
            match section {
                Section::Unchanged { contents } => {
                    const CONTEXT: usize = 2;

                    // Add the trailing context for the previous section (if any).
                    if section_num > 0 {
                        let end_index = min(CONTEXT, contents.len());
                        for (i, line) in contents[..end_index].iter().enumerate() {
                            file_view.add_child(
                                " ",
                                TextView::new(format!("  {} {}", line_num + i, line)),
                            );
                        }
                    }

                    // Add vertical ellipsis between sections.
                    if section_num > 0 && section_num + 1 < sections.len() {
                        file_view.add_child("", TextView::new(":"));
                    }

                    // Add the leading context for the next section (if any).
                    if section_num + 1 < sections.len() {
                        let start_index = contents.len().saturating_sub(CONTEXT);
                        for (i, line) in contents[start_index..].iter().enumerate() {
                            file_view.add_child(
                                "",
                                TextView::new(format!("  {} {}", line_num + start_index + i, line)),
                            );
                        }
                    }

                    line_num += contents.len();
                }

                Section::Changed { before, after } => {
                    local_changed_section_num += 1;
                    *global_changed_section_num += 1;
                    let description = format!(
                        "section {}/{} in current file, {}/{} total",
                        local_changed_section_num,
                        local_num_changed_sections,
                        global_changed_section_num,
                        global_num_changed_sections
                    );

                    self.make_changed_section_views(
                        main_tx.clone(),
                        &mut file_view,
                        file_num,
                        section_num,
                        description,
                        before,
                        after,
                    );
                    line_num += before.len();
                }

                Section::FileMode {
                    is_selected: _,
                    before: _,
                    after: _,
                } => {
                    local_changed_section_num += 1;
                    *global_changed_section_num += 1;
                    let description = format!(
                        "section {}/{} in current file, {}/{} total",
                        local_changed_section_num,
                        local_num_changed_sections,
                        global_changed_section_num,
                        global_num_changed_sections
                    );

                    // @nocommit
                    // self.make_changed_section_views(
                    //     main_tx.clone(),
                    //     &mut file_view,
                    //     file_num,
                    //     section_num,
                    //     description,
                    //     before,
                    //     after,
                    // );
                }
            }
        }

        file_view
    }

    fn make_changed_section_views(
        &self,
        main_tx: Sender<Message>,
        view: &mut ListView2,
        file_num: usize,
        section_num: usize,
        section_description: String,
        before: &[SectionChangedLine],
        after: &[SectionChangedLine],
    ) {
        let mut section_view = ListView2::new();
        let section_key = SectionKey {
            file_num,
            section_num,
        };

        for (section_line_num, section_changed_line) in before.iter().enumerate() {
            let section_line_key = SectionLineKey {
                file_num,
                section_num,
                section_type: SectionChangedLineType::Before,
                section_line_num,
            };
            section_view.add_child(
                "    ",
                self.make_changed_line_view(
                    main_tx.clone(),
                    section_line_key,
                    section_changed_line,
                ),
            );
        }

        for (section_line_num, section_changed_line) in after.iter().enumerate() {
            let section_line_key = SectionLineKey {
                file_num,
                section_num,
                section_type: SectionChangedLineType::After,
                section_line_num,
            };
            section_view.add_child(
                "    ",
                self.make_changed_line_view(
                    main_tx.clone(),
                    section_line_key,
                    section_changed_line,
                ),
            );
        }

        view.add_child(
            "  ",
            LinearLayout::horizontal()
                .child(
                    TristateBox::new()
                        .with_state(
                            all_are_same_value(before.iter().chain(after.iter()).map(
                                |SectionChangedLine {
                                     is_selected,
                                     line: _,
                                 }| { *is_selected },
                            ))
                            .into_tristate(),
                        )
                        .on_change({
                            let section_key = SectionKey {
                                file_num,
                                section_num,
                            };
                            let main_tx = main_tx;
                            move |_, new_value| {
                                if main_tx
                                    .send(Message::ToggleHunk(section_key, new_value))
                                    .is_err()
                                {
                                    // Do nothing.
                                }
                            }
                        })
                        .with_name(section_key.view_id()),
                )
                .child(TextView::new({
                    let mut s = StyledString::new();
                    s.append_plain(" ");
                    s.append_plain(section_description);
                    s
                })),
        );
        view.add_child("", HideableView::new(section_view));
    }

    fn make_changed_line_view(
        &self,
        main_tx: Sender<Message>,
        section_line_key: SectionLineKey,
        section_changed_line: &SectionChangedLine,
    ) -> impl View {
        let SectionChangedLine { is_selected, line } = section_changed_line;

        let line_contents = {
            let (line, style) = match section_line_key.section_type {
                SectionChangedLineType::Before => (format!(" -{}", line), BaseColor::Red.dark()),
                SectionChangedLineType::After => (format!(" +{}", line), BaseColor::Green.dark()),
            };
            let mut s = StyledString::new();
            s.append_styled(line, style);
            s
        };

        LinearLayout::horizontal()
            .child(
                Checkbox::new()
                    .with_checked(*is_selected)
                    .on_change({
                        move |_, is_selected| {
                            if main_tx
                                .send(Message::ToggleHunkLine(section_line_key, is_selected))
                                .is_err()
                            {
                                // Do nothing.
                            }
                        }
                    })
                    .with_name(section_line_key.view_id()),
            )
            .child(TextView::new(line_contents))
    }

    fn toggle_file(
        &mut self,
        siv: &mut CursiveRunner<CursiveRunnable>,
        file_key: FileKey,
        new_value: Tristate,
    ) {
        let FileKey { file_num } = file_key;
        let (_path, file_state) = &mut self.state.file_states[file_num];

        let new_value = match new_value {
            Tristate::Unchecked => false,
            Tristate::Partial => {
                // Shouldn't happen.
                true
            }
            Tristate::Checked => true,
        };

        let FileState {
            file_mode: _,
            sections,
        } = file_state;
        for (section_num, section) in sections.iter_mut().enumerate() {
            match section {
                Section::Unchanged { contents: _ } => {
                    // Do nothing.
                }
                Section::Changed { before, after } => {
                    for (
                        section_line_num,
                        SectionChangedLine {
                            is_selected,
                            line: _,
                        },
                    ) in before.iter_mut().enumerate()
                    {
                        *is_selected = new_value;
                        let section_line_key = SectionLineKey {
                            file_num,
                            section_num,
                            section_type: SectionChangedLineType::Before,
                            section_line_num,
                        };
                        siv.call_on_name(&section_line_key.view_id(), |checkbox: &mut Checkbox| {
                            checkbox.set_checked(new_value)
                        });
                    }

                    for (
                        section_line_num,
                        SectionChangedLine {
                            is_selected,
                            line: _,
                        },
                    ) in after.iter_mut().enumerate()
                    {
                        *is_selected = new_value;
                        let section_line_key = SectionLineKey {
                            file_num,
                            section_num,
                            section_type: SectionChangedLineType::After,
                            section_line_num,
                        };
                        siv.call_on_name(&section_line_key.view_id(), |checkbox: &mut Checkbox| {
                            checkbox.set_checked(new_value)
                        });
                    }
                }
                Section::FileMode {
                    is_selected: _,
                    before: _,
                    after: _,
                } => {
                    unimplemented!("toggle_file for Section::FileMode");
                }
            }

            let section_key = SectionKey {
                file_num,
                section_num,
            };
            siv.call_on_name(&section_key.view_id(), |tristate_box: &mut TristateBox| {
                tristate_box.set_state(if new_value {
                    Tristate::Checked
                } else {
                    Tristate::Unchecked
                });
            });
        }
    }

    fn toggle_section(
        &mut self,
        siv: &mut CursiveRunner<CursiveRunnable>,
        section_key: SectionKey,
        new_value: Tristate,
    ) {
        let SectionKey {
            file_num,
            section_num,
        } = section_key;

        let new_value = match new_value {
            Tristate::Unchecked => false,
            Tristate::Partial => {
                // Shouldn't happen.
                true
            }
            Tristate::Checked => true,
        };

        let (before, after) = {
            let (
                path,
                FileState {
                    file_mode: _,
                    sections,
                },
            ) = &mut self.state.file_states[file_num];
            match &mut sections[section_num] {
                Section::Unchanged { contents } => {
                    error!(
                        ?section_num,
                        ?path,
                        ?contents,
                        "Invalid section num to change"
                    );
                    panic!("Invalid section num to change");
                }
                Section::Changed { before, after } => (before, after),
                Section::FileMode {
                    is_selected: _,
                    before: _,
                    after: _,
                } => {
                    unimplemented!("toggle_section for Section::FileMode");
                }
            }
        };

        // Update child checkboxes.
        for changed_line in before.iter_mut() {
            changed_line.is_selected = new_value;
        }
        for changed_line in after.iter_mut() {
            changed_line.is_selected = new_value;
        }
        let section_line_keys = (0..before.len())
            .map(|section_line_num| SectionLineKey {
                file_num,
                section_num,
                section_type: SectionChangedLineType::Before,
                section_line_num,
            })
            .chain((0..after.len()).map(|section_line_num| SectionLineKey {
                file_num,
                section_num,
                section_type: SectionChangedLineType::After,
                section_line_num,
            }));
        for section_line_key in section_line_keys {
            siv.call_on_name(&section_line_key.view_id(), |checkbox: &mut Checkbox| {
                checkbox.set_checked(new_value);
            });
        }

        self.refresh_file(siv, FileKey { file_num });
    }

    fn toggle_section_line(
        &mut self,
        siv: &mut CursiveRunner<CursiveRunnable>,
        section_line_key: SectionLineKey,
        new_value: bool,
    ) {
        let SectionLineKey {
            file_num,
            section_num,
            section_type,
            section_line_num,
        } = section_line_key;

        let (
            path,
            FileState {
                file_mode: _,
                sections,
            },
        ) = &mut self.state.file_states[file_num];
        let section = &mut sections[section_num];
        let section_changed_lines = match (section, section_type) {
            (Section::Unchanged { contents }, _) => {
                error!(
                    ?section_num,
                    ?path,
                    ?contents,
                    "Invalid section num to change"
                );
                panic!("Invalid section num to change");
            }
            (Section::Changed { before, .. }, SectionChangedLineType::Before) => before,
            (Section::Changed { after, .. }, SectionChangedLineType::After) => after,
            (
                Section::FileMode {
                    is_selected: _,
                    before: _,
                    after: _,
                },
                _,
            ) => unimplemented!("toggle_section_line for Section::FileMode"),
        };
        section_changed_lines[section_line_num].is_selected = new_value;

        self.refresh_section(
            siv,
            SectionKey {
                file_num,
                section_num,
            },
        );
        self.refresh_file(siv, FileKey { file_num });
    }

    fn refresh_section(
        &mut self,
        siv: &mut CursiveRunner<CursiveRunnable>,
        section_key: SectionKey,
    ) {
        let SectionKey {
            file_num,
            section_num,
        } = section_key;
        let (
            _path,
            FileState {
                file_mode: _,
                sections,
            },
        ) = &mut self.state.file_states[file_num];

        let section_selections = iter_section_selections(&sections[section_num]);
        let section_new_value = all_are_same_value(section_selections).into_tristate();
        let section_key = SectionKey {
            file_num,
            section_num,
        };
        siv.call_on_name(&section_key.view_id(), |tristate_box: &mut TristateBox| {
            tristate_box.set_state(section_new_value);
        });
    }

    fn refresh_file(&mut self, siv: &mut CursiveRunner<CursiveRunnable>, file_key: FileKey) {
        let FileKey { file_num } = file_key;
        let file_state = &mut self.state.file_states[file_num].1;

        let file_selections = iter_file_selections(file_state);
        let file_new_value = all_are_same_value(file_selections).into_tristate();
        siv.call_on_name(&file_key.view_id(), |tristate_box: &mut TristateBox| {
            tristate_box.set_state(file_new_value);
        });
    }
}

fn iter_file_selections<'a>(file_state: &'a FileState) -> impl Iterator<Item = bool> + 'a {
    let FileState {
        file_mode: _,
        sections,
    } = file_state;
    sections.iter().flat_map(iter_section_selections)
}

fn iter_section_selections<'a>(section: &'a Section) -> impl Iterator<Item = bool> + 'a {
    let iter: Box<dyn Iterator<Item = bool>> = match section {
        Section::Changed { before, after } => Box::new(
            before
                .iter()
                .map(|changed_line| changed_line.is_selected)
                .chain(after.iter().map(|changed_line| changed_line.is_selected)),
        ),
        Section::Unchanged { contents: _ } => Box::new(std::iter::empty()),
        Section::FileMode {
            is_selected,
            before: _,
            after: _,
        } => Box::new(vec![*is_selected].into_iter()),
    };
    iter
}

#[derive(Clone, Debug)]
pub enum Message {
    Init,
    ToggleFile(FileKey, Tristate),
    ToggleHunk(SectionKey, Tristate),
    ToggleHunkLine(SectionLineKey, bool),
    Confirm,
    Quit,
}

impl<'a> EventDrivenCursiveApp for Recorder<'a> {
    type Message = Message;

    type Return = Result<RecordState<'a>, RecordError>;

    fn get_init_message(&self) -> Self::Message {
        Message::Init
    }

    fn get_key_bindings(&self) -> Vec<(Event, Self::Message)> {
        vec![
            ('c'.into(), Message::Confirm),
            ('C'.into(), Message::Confirm),
            ('q'.into(), Message::Quit),
            ('Q'.into(), Message::Quit),
        ]
    }

    fn handle_message(
        &mut self,
        siv: &mut CursiveRunner<CursiveRunnable>,
        main_tx: Sender<Self::Message>,
        message: Self::Message,
    ) {
        match message {
            Message::Init => {
                let main_view = self.make_main_view(main_tx);
                siv.add_layer(ScrollView::new(
                    // NB: you can't add `min_width` to the `ScrollView` itself,
                    // or else the scrollbar stops responding to clicks and
                    // drags.
                    main_view.min_width(80),
                ));
            }

            Message::ToggleFile(file_key, new_value) => {
                self.toggle_file(siv, file_key, new_value);
            }

            Message::ToggleHunk(section_key, new_value) => {
                self.toggle_section(siv, section_key, new_value);
            }

            Message::ToggleHunkLine(section_line_key, new_value) => {
                self.toggle_section_line(siv, section_line_key, new_value);
            }

            Message::Confirm => {
                self.did_user_confirm_exit = true;
                siv.quit();
            }

            Message::Quit => {
                let has_changes = {
                    let RecordState { file_states } = &self.state;
                    let changed_lines = file_states
                        .iter()
                        .flat_map(|(_, file_state)| iter_file_selections(file_state));
                    match all_are_same_value(changed_lines) {
                        SameValueResult::Empty | SameValueResult::AllSame(_) => false,
                        SameValueResult::SomeDifferent => true,
                    }
                };

                if has_changes {
                    siv.add_layer(
                        Dialog::text(
                            "Are you sure you want to quit? Your selections will be lost.",
                        )
                        .button("Ok", |siv| {
                            siv.quit();
                        })
                        .dismiss_button("Cancel"),
                    );
                } else {
                    siv.quit();
                }
            }
        }
    }

    fn finish(self) -> Self::Return {
        if self.did_user_confirm_exit {
            Ok(self.state)
        } else {
            Err(RecordError::Cancelled)
        }
    }
}

enum SameValueResult<T> {
    Empty,
    AllSame(T),
    SomeDifferent,
}

impl SameValueResult<bool> {
    fn into_tristate(self) -> Tristate {
        match self {
            SameValueResult::Empty | SameValueResult::AllSame(true) => Tristate::Checked,
            SameValueResult::AllSame(false) => Tristate::Unchecked,
            SameValueResult::SomeDifferent => Tristate::Partial,
        }
    }
}

fn all_are_same_value<Iter, Item>(iter: Iter) -> SameValueResult<Item>
where
    Iter: IntoIterator<Item = Item>,
    Item: Eq,
{
    let mut first_value = None;
    for value in iter {
        match &first_value {
            Some(first_value) => {
                if &value != first_value {
                    return SameValueResult::SomeDifferent;
                }
            }
            None => {
                first_value = Some(value);
            }
        }
    }

    match first_value {
        Some(value) => SameValueResult::AllSame(value),
        None => SameValueResult::Empty,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::{borrow::Cow, convert::Infallible};

    use cursive::event::Key;

    use crate::cursive_utils::testing::{
        screen_to_string, CursiveTestingBackend, CursiveTestingEvent,
    };

    use super::*;

    fn run_test(
        state: RecordState,
        events: Vec<CursiveTestingEvent>,
    ) -> Result<RecordState, RecordError> {
        let siv = CursiveRunnable::new::<Infallible, _>(move || {
            Ok(CursiveTestingBackend::init(events.clone()))
        });
        let recorder = Recorder::new(state);
        recorder.run(siv.into_runner())
    }

    fn example_record_state() -> RecordState<'static> {
        RecordState {
            file_states: vec![(
                PathBuf::from("foo"),
                FileState {
                    file_mode: None,
                    sections: vec![
                        Section::Unchanged {
                            contents: vec![
                                Cow::Borrowed("unchanged 1\n"),
                                Cow::Borrowed("unchanged 2\n"),
                            ],
                        },
                        Section::Changed {
                            before: vec![
                                SectionChangedLine {
                                    is_selected: true,
                                    line: Cow::Borrowed("before 1\n"),
                                },
                                SectionChangedLine {
                                    is_selected: true,
                                    line: Cow::Borrowed("before 2\n"),
                                },
                            ],
                            after: vec![
                                SectionChangedLine {
                                    is_selected: true,
                                    line: Cow::Borrowed("after 1\n"),
                                },
                                SectionChangedLine {
                                    is_selected: false,
                                    line: Cow::Borrowed("after 2\n"),
                                },
                            ],
                        },
                    ],
                },
            )],
        }
    }

    #[test]
    fn test_cancel() {
        let screenshot1 = Default::default();
        let result = run_test(
            example_record_state(),
            vec![
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
                CursiveTestingEvent::Event('q'.into()),
                CursiveTestingEvent::Event(Key::Enter.into()),
            ],
        );
        insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
        [~] foo
                              1 unchanged 1
                              2 unchanged 2
                              [~] section 1/1 in current file, 1/1 total
                                [X] -before 1
                                [X] -before 2
                                [X] +after 1
                                [ ] +after 2
        "###);
        insta::assert_debug_snapshot!(result, @r###"
        Err(
            Cancelled,
        )
        "###);
    }

    #[test]
    fn test_cancel_no_confirm() {
        let screenshot1 = Default::default();
        let result = run_test(
            example_record_state(),
            vec![
                CursiveTestingEvent::Event(Key::Down.into()), // section
                CursiveTestingEvent::Event(Key::Down.into()), // before 1
                CursiveTestingEvent::Event(Key::Down.into()), // before 2
                CursiveTestingEvent::Event(Key::Down.into()), // after 1
                CursiveTestingEvent::Event(Key::Down.into()), // after 2
                CursiveTestingEvent::Event(' '.into()),
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
                CursiveTestingEvent::Event('q'.into()), // don't press enter to confirm
            ],
        );
        insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
        [X] foo
                              1 unchanged 1
                              2 unchanged 2
                              [X] section 1/1 in current file, 1/1 total
                                [X] -before 1
                                [X] -before 2
                                [X] +after 1
                                [X] +after 2
        "###);
        insta::assert_debug_snapshot!(result, @r###"
        Err(
            Cancelled,
        )
        "###);
    }

    #[test]
    fn test_section_toggle() {
        let screenshot1 = Default::default();
        let screenshot2 = Default::default();
        let screenshot3 = Default::default();
        let screenshot4 = Default::default();
        let result = run_test(
            example_record_state(),
            vec![
                CursiveTestingEvent::Event(Key::Down.into()), // move to section
                CursiveTestingEvent::Event(' '.into()),       // toggle section
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
                CursiveTestingEvent::Event(' '.into()), // toggle section
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot2)),
                CursiveTestingEvent::Event(Key::Down.into()), // move to line
                CursiveTestingEvent::Event(' '.into()),       // toggle line
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot3)),
                CursiveTestingEvent::Event(Key::Up.into()), // move to section
                CursiveTestingEvent::Event(' '.into()),     // toggle section
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot4)),
                CursiveTestingEvent::Event('c'.into()),
            ],
        );

        insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
        [X] foo
                              1 unchanged 1
                              2 unchanged 2
                              [X] section 1/1 in current file, 1/1 total
                                [X] -before 1
                                [X] -before 2
                                [X] +after 1
                                [X] +after 2
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot2), @r###"
        [ ] foo
                              1 unchanged 1
                              2 unchanged 2
                              [ ] section 1/1 in current file, 1/1 total
                                [ ] -before 1
                                [ ] -before 2
                                [ ] +after 1
                                [ ] +after 2
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot3), @r###"
        [~] foo
                              1 unchanged 1
                              2 unchanged 2
                              [~] section 1/1 in current file, 1/1 total
                                [X] -before 1
                                [ ] -before 2
                                [ ] +after 1
                                [ ] +after 2
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot4), @r###"
        [X] foo
                              1 unchanged 1
                              2 unchanged 2
                              [X] section 1/1 in current file, 1/1 total
                                [X] -before 1
                                [X] -before 2
                                [X] +after 1
                                [X] +after 2
        "###);
        insta::assert_debug_snapshot!(result, @r###"
        Ok(
            RecordState {
                file_states: [
                    (
                        "foo",
                        FileState {
                            file_mode: None,
                            sections: [
                                Unchanged {
                                    contents: [
                                        "unchanged 1\n",
                                        "unchanged 2\n",
                                    ],
                                },
                                Changed {
                                    before: [
                                        SectionChangedLine {
                                            is_selected: true,
                                            line: "before 1\n",
                                        },
                                        SectionChangedLine {
                                            is_selected: true,
                                            line: "before 2\n",
                                        },
                                    ],
                                    after: [
                                        SectionChangedLine {
                                            is_selected: true,
                                            line: "after 1\n",
                                        },
                                        SectionChangedLine {
                                            is_selected: true,
                                            line: "after 2\n",
                                        },
                                    ],
                                },
                            ],
                        },
                    ),
                ],
            },
        )
        "###);
    }

    #[test]
    fn test_file_toggle() {
        let screenshot1 = Default::default();
        let screenshot2 = Default::default();
        let screenshot3 = Default::default();
        let screenshot4 = Default::default();
        let result = run_test(
            example_record_state(),
            vec![
                CursiveTestingEvent::Event(' '.into()), // toggle file
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
                CursiveTestingEvent::Event(Key::Down.into()), // move to section
                CursiveTestingEvent::Event(' '.into()),       // toggle section
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot2)),
                CursiveTestingEvent::Event(Key::Down.into()), // move to line
                CursiveTestingEvent::Event(' '.into()),       // toggle line
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot3)),
                CursiveTestingEvent::Event(Key::Up.into()),
                CursiveTestingEvent::Event(Key::Up.into()), // move to file
                CursiveTestingEvent::Event(' '.into()),     // toggle file
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot4)),
                CursiveTestingEvent::Event('c'.into()),
            ],
        );

        insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
        [X] foo
                              1 unchanged 1
                              2 unchanged 2
                              [X] section 1/1 in current file, 1/1 total
                                [X] -before 1
                                [X] -before 2
                                [X] +after 1
                                [X] +after 2
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot2), @r###"
        [ ] foo
                              1 unchanged 1
                              2 unchanged 2
                              [ ] section 1/1 in current file, 1/1 total
                                [ ] -before 1
                                [ ] -before 2
                                [ ] +after 1
                                [ ] +after 2
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot3), @r###"
        [~] foo
                              1 unchanged 1
                              2 unchanged 2
                              [~] section 1/1 in current file, 1/1 total
                                [X] -before 1
                                [ ] -before 2
                                [ ] +after 1
                                [ ] +after 2
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot4), @r###"
        [X] foo
                              1 unchanged 1
                              2 unchanged 2
                              [X] section 1/1 in current file, 1/1 total
                                [X] -before 1
                                [X] -before 2
                                [X] +after 1
                                [X] +after 2
        "###);
        insta::assert_debug_snapshot!(result, @r###"
        Ok(
            RecordState {
                file_states: [
                    (
                        "foo",
                        FileState {
                            file_mode: None,
                            sections: [
                                Unchanged {
                                    contents: [
                                        "unchanged 1\n",
                                        "unchanged 2\n",
                                    ],
                                },
                                Changed {
                                    before: [
                                        SectionChangedLine {
                                            is_selected: true,
                                            line: "before 1\n",
                                        },
                                        SectionChangedLine {
                                            is_selected: true,
                                            line: "before 2\n",
                                        },
                                    ],
                                    after: [
                                        SectionChangedLine {
                                            is_selected: true,
                                            line: "after 1\n",
                                        },
                                        SectionChangedLine {
                                            is_selected: true,
                                            line: "after 2\n",
                                        },
                                    ],
                                },
                            ],
                        },
                    ),
                ],
            },
        )
        "###);
    }

    #[test]
    fn test_initial_tristate_states() {
        let state = {
            let mut state = example_record_state();
            let (
                _path,
                FileState {
                    file_mode: _,
                    sections,
                },
            ) = &mut state.file_states[0];
            for section in sections {
                if let Section::Changed { before, after: _ } = section {
                    before[0].is_selected = true;
                }
            }
            state
        };

        let screenshot1 = Default::default();
        let result = run_test(
            state,
            vec![
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
                CursiveTestingEvent::Event('q'.into()),
                CursiveTestingEvent::Event(Key::Enter.into()),
            ],
        );
        insta::assert_debug_snapshot!(result, @r###"
        Err(
            Cancelled,
        )
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
        [~] foo
                              1 unchanged 1
                              2 unchanged 2
                              [~] section 1/1 in current file, 1/1 total
                                [X] -before 1
                                [X] -before 2
                                [X] +after 1
                                [ ] +after 2
        "###);
    }

    #[test]
    fn test_context() {
        let state = RecordState {
            file_states: vec![(
                PathBuf::from("foo"),
                FileState {
                    file_mode: None,
                    sections: vec![
                        Section::Unchanged {
                            contents: vec![
                                Cow::Borrowed("foo"),
                                Cow::Borrowed("bar"),
                                Cow::Borrowed("baz"),
                                Cow::Borrowed("qux"),
                            ],
                        },
                        Section::Changed {
                            before: vec![SectionChangedLine {
                                is_selected: false,
                                line: Cow::Borrowed("changed 1"),
                            }],
                            after: vec![
                                SectionChangedLine {
                                    is_selected: false,
                                    line: Cow::Borrowed("changed 2"),
                                },
                                SectionChangedLine {
                                    is_selected: false,
                                    line: Cow::Borrowed("changed 3"),
                                },
                            ],
                        },
                        Section::Unchanged {
                            contents: vec![
                                Cow::Borrowed("foo"),
                                Cow::Borrowed("bar"),
                                Cow::Borrowed("baz"),
                                Cow::Borrowed("qux"),
                            ],
                        },
                    ],
                },
            )],
        };

        let screenshot = Default::default();
        let result = run_test(
            state,
            vec![
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot)),
                CursiveTestingEvent::Event('q'.into()),
            ],
        );

        insta::assert_debug_snapshot!(result, @r###"
        Err(
            Cancelled,
        )
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot), @r###"
        [ ] foo
                              3 baz
                              4 qux
                              [ ] section 1/1 in current file, 1/1 total
                                [ ] -changed 1
                                [ ] +changed 2
                                [ ] +changed 3
                              6 foo
                              7 bar
        "###);
    }

    #[test]
    fn test_render_multiple_sections() -> eyre::Result<()> {
        let mut state = example_record_state();
        let sections = {
            let (_, FileState { sections, .. }) = &state.file_states[0];
            sections.clone()
        };
        state.file_states[0].1.sections = [sections.clone(), sections].concat();

        let screenshot = Default::default();
        let result = run_test(
            state,
            vec![
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot)),
                CursiveTestingEvent::Event('q'.into()),
                CursiveTestingEvent::Event(Key::Enter.into()),
            ],
        );

        insta::assert_debug_snapshot!(result, @r###"
        Err(
            Cancelled,
        )
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot), @r###"
        [~] foo
                              1 unchanged 1
                              2 unchanged 2
                              [~] section 1/2 in current file, 1/2 total
                                [X] -before 1
                                [X] -before 2
                                [X] +after 1
                                [ ] +after 2
                              5 unchanged 1
                              6 unchanged 2
                            :
                              5 unchanged 1
                              6 unchanged 2
                              [~] section 2/2 in current file, 2/2 total
                                [X] -before 1
                                [X] -before 2
                                [X] +after 1
                                [ ] +after 2
        "###);

        Ok(())
    }

    #[test]
    fn test_file_mode_section() -> eyre::Result<()> {
        let state = RecordState {
            file_states: vec![(
                "foo".into(),
                FileState {
                    file_mode: Some(0o100644),
                    sections: vec![Section::FileMode {
                        is_selected: false,
                        before: 0o100644,
                        after: 0o100755,
                    }],
                },
            )],
        };

        let screenshot = Default::default();
        let result = run_test(
            state,
            vec![
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot)),
                CursiveTestingEvent::Event('q'.into()),
                CursiveTestingEvent::Event(Key::Enter.into()),
            ],
        );

        insta::assert_snapshot!(screen_to_string(&screenshot), @"");

        Ok(())
    }
}
