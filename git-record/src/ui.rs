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
use crate::{FileHunks, Hunk, HunkChangedLine, RecordError, RecordState};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum HunkType {
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
pub struct HunkKey {
    file_num: usize,
    hunk_num: usize,
}

impl HunkKey {
    fn view_id(&self) -> String {
        let Self { file_num, hunk_num } = self;
        format!("HunkKey({},{})", *file_num, hunk_num)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct HunkLineKey {
    file_num: usize,
    hunk_num: usize,
    hunk_type: HunkType,
    hunk_line_num: usize,
}

impl HunkLineKey {
    fn view_id(&self) -> String {
        let Self {
            file_num,
            hunk_num,
            hunk_type,
            hunk_line_num,
        } = self;
        format!(
            "HunkLineKey({},{},{},{})",
            file_num,
            hunk_num,
            match hunk_type {
                HunkType::Before => "B",
                HunkType::After => "A",
            },
            hunk_line_num
        )
    }
}

/// UI component to record the user's changes.
pub struct Recorder {
    did_user_confirm_exit: bool,
    state: RecordState,
}

impl Recorder {
    /// Constructor.
    pub fn new(state: RecordState) -> Self {
        Self {
            did_user_confirm_exit: false,
            state,
        }
    }

    /// Run the terminal user interface and have the user interactively select
    /// changes.
    pub fn run(self, siv: CursiveRunner<CursiveRunnable>) -> Result<RecordState, RecordError> {
        EventDrivenCursiveAppExt::run(self, siv)
    }

    fn make_main_view(&self, main_tx: Sender<Message>) -> impl View {
        let mut view = LinearLayout::vertical();

        let RecordState { files } = &self.state;

        let count_changed_hunks = |&FileHunks { ref hunks }| {
            hunks
                .iter()
                .filter(|hunk| match hunk {
                    Hunk::Unchanged { .. } => false,
                    Hunk::Changed { .. } => true,
                })
                .count()
        };
        let global_num_changed_hunks: usize = files
            .iter()
            .map(|(_path, hunks)| count_changed_hunks(hunks))
            .sum();
        let mut global_changed_hunk_num = 0;

        for (file_num, (path, file_hunks)) in files.iter().enumerate() {
            let file_key = FileKey { file_num };
            let mut file_view = LinearLayout::vertical();
            let mut line_num: usize = 0;
            let local_num_changed_hunks = count_changed_hunks(file_hunks);

            let FileHunks { hunks } = file_hunks;
            for (hunk_num, hunk) in hunks.iter().enumerate() {
                match hunk {
                    Hunk::Unchanged { contents } => {
                        for line in contents {
                            line_num += 1;
                            file_view.add_child(TextView::new(format!("  {} {}", line_num, line)));
                        }
                    }

                    Hunk::Changed { before, after } => {
                        global_changed_hunk_num += 1;
                        let description = format!(
                            "hunk {}/{} in current file, {}/{} total",
                            hunk_num,
                            local_num_changed_hunks,
                            global_changed_hunk_num,
                            global_num_changed_hunks
                        );

                        self.make_changed_hunk_views(
                            main_tx.clone(),
                            &mut file_view,
                            &mut line_num,
                            file_num,
                            hunk_num,
                            description,
                            before,
                            after,
                        );
                    }
                }
            }

            view.add_child(self.make_file_view(main_tx.clone(), path, file_key, file_hunks));
            view.add_child(file_view);
            if file_num + 1 < files.len() {
                // Render a spacer line. Note that an empty string won't render an
                // empty line.
                view.add_child(TextView::new(" "));
            }
        }

        view
    }

    fn make_file_view(
        &self,
        main_tx: Sender<Message>,
        path: &Path,
        file_key: FileKey,
        file_hunks: &FileHunks,
    ) -> impl View {
        LinearLayout::horizontal()
            .child(
                TristateBox::new()
                    .with_state({
                        all_are_same_value(iter_file_changed_lines(file_hunks).map(
                            |(
                                _hunk_type,
                                HunkChangedLine {
                                    is_selected,
                                    line: _,
                                },
                            )| *is_selected,
                        ))
                        .into_tristate()
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
                s.append_plain(" /");
                s.append_styled(path.to_string_lossy(), Effect::Bold);
                s
            }))
    }

    fn make_changed_hunk_views(
        &self,
        main_tx: Sender<Message>,
        view: &mut LinearLayout,
        line_num: &mut usize,
        file_num: usize,
        hunk_num: usize,
        hunk_description: String,
        before: &[HunkChangedLine],
        after: &[HunkChangedLine],
    ) {
        let mut hunk_view = LinearLayout::vertical();
        let hunk_key = HunkKey { file_num, hunk_num };

        for (hunk_line_num, hunk_changed_line) in before.iter().enumerate() {
            let hunk_line_key = HunkLineKey {
                file_num,
                hunk_num,
                hunk_type: HunkType::Before,
                hunk_line_num,
            };
            hunk_view.add_child(self.make_changed_line_view(
                main_tx.clone(),
                line_num,
                hunk_line_key,
                hunk_changed_line,
            ));
        }

        for (hunk_line_num, hunk_changed_line) in after.iter().enumerate() {
            let hunk_line_key = HunkLineKey {
                file_num,
                hunk_num,
                hunk_type: HunkType::After,
                hunk_line_num,
            };
            hunk_view.add_child(self.make_changed_line_view(
                main_tx.clone(),
                line_num,
                hunk_line_key,
                hunk_changed_line,
            ));
        }

        view.add_child(
            LinearLayout::horizontal()
                .child(TextView::new("  "))
                .child(
                    TristateBox::new()
                        .with_state(
                            all_are_same_value(before.iter().chain(after.iter()).map(
                                |HunkChangedLine {
                                     is_selected,
                                     line: _,
                                 }| { *is_selected },
                            ))
                            .into_tristate(),
                        )
                        .on_change({
                            let hunk_key = HunkKey { file_num, hunk_num };
                            let main_tx = main_tx;
                            move |_, new_value| {
                                if main_tx
                                    .send(Message::ToggleHunk(hunk_key, new_value))
                                    .is_err()
                                {
                                    // Do nothing.
                                }
                            }
                        })
                        .with_name(hunk_key.view_id()),
                )
                .child(TextView::new({
                    let mut s = StyledString::new();
                    s.append_plain(" ");
                    s.append_plain(hunk_description);
                    s
                })),
        );
        view.add_child(HideableView::new(hunk_view));
    }

    fn make_changed_line_view(
        &self,
        main_tx: Sender<Message>,
        line_num: &mut usize,
        hunk_line_key: HunkLineKey,
        hunk_changed_line: &HunkChangedLine,
    ) -> impl View {
        let HunkChangedLine { is_selected, line } = hunk_changed_line;

        *line_num += 1;
        let line_contents = {
            let (line, style) = match hunk_line_key.hunk_type {
                HunkType::Before => (format!(" -{}", line), BaseColor::Red.dark()),
                HunkType::After => (format!(" +{}", line), BaseColor::Green.dark()),
            };
            let mut s = StyledString::new();
            s.append_styled(line, style);
            s
        };

        LinearLayout::horizontal()
            .child(TextView::new("    "))
            .child(
                Checkbox::new()
                    .with_checked(*is_selected)
                    .on_change({
                        let main_tx = main_tx.clone();
                        move |_, is_selected| {
                            if main_tx
                                .send(Message::ToggleHunkLine(hunk_line_key, is_selected))
                                .is_err()
                            {
                                // Do nothing.
                            }
                        }
                    })
                    .with_name(hunk_line_key.view_id()),
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
        let (_path, file_hunks) = &mut self.state.files[file_num];

        let new_value = match new_value {
            Tristate::Unchecked => false,
            Tristate::Partial => {
                // Shouldn't happen.
                true
            }
            Tristate::Checked => true,
        };

        let FileHunks { hunks } = file_hunks;
        for (hunk_num, hunk) in hunks.iter_mut().enumerate() {
            match hunk {
                Hunk::Unchanged { contents: _ } => {
                    // Do nothing.
                }
                Hunk::Changed { before, after } => {
                    for (
                        hunk_line_num,
                        HunkChangedLine {
                            is_selected,
                            line: _,
                        },
                    ) in before.iter_mut().enumerate()
                    {
                        *is_selected = new_value;
                        let hunk_line_key = HunkLineKey {
                            file_num,
                            hunk_num,
                            hunk_type: HunkType::Before,
                            hunk_line_num,
                        };
                        siv.call_on_name(&hunk_line_key.view_id(), |checkbox: &mut Checkbox| {
                            checkbox.set_checked(new_value)
                        });
                    }

                    for (
                        hunk_line_num,
                        HunkChangedLine {
                            is_selected,
                            line: _,
                        },
                    ) in after.iter_mut().enumerate()
                    {
                        *is_selected = new_value;
                        let hunk_line_key = HunkLineKey {
                            file_num,
                            hunk_num,
                            hunk_type: HunkType::After,
                            hunk_line_num,
                        };
                        siv.call_on_name(&hunk_line_key.view_id(), |checkbox: &mut Checkbox| {
                            checkbox.set_checked(new_value)
                        });
                    }
                }
            }

            let hunk_key = HunkKey { file_num, hunk_num };
            siv.call_on_name(&hunk_key.view_id(), |tristate_box: &mut TristateBox| {
                tristate_box.set_state(if new_value {
                    Tristate::Checked
                } else {
                    Tristate::Unchecked
                });
            });
        }
    }

    fn toggle_hunk(
        &mut self,
        siv: &mut CursiveRunner<CursiveRunnable>,
        hunk_key: HunkKey,
        new_value: Tristate,
    ) {
        let HunkKey { file_num, hunk_num } = hunk_key;

        let new_value = match new_value {
            Tristate::Unchecked => false,
            Tristate::Partial => {
                // Shouldn't happen.
                true
            }
            Tristate::Checked => true,
        };

        let (before, after) = {
            let (path, hunks) = &mut self.state.files[file_num];
            let hunk = {
                let FileHunks { hunks } = hunks;
                &mut hunks[hunk_num]
            };

            match hunk {
                Hunk::Unchanged { contents } => {
                    error!(?hunk_num, ?path, ?contents, "Invalid hunk num to change");
                    panic!("Invalid hunk num to change");
                }
                Hunk::Changed { before, after } => (before, after),
            }
        };

        // Update child checkboxes.
        for changed_line in before.iter_mut() {
            changed_line.is_selected = new_value;
        }
        for changed_line in after.iter_mut() {
            changed_line.is_selected = new_value;
        }
        let hunk_line_keys = (0..before.len())
            .map(|hunk_line_num| HunkLineKey {
                file_num,
                hunk_num,
                hunk_type: HunkType::Before,
                hunk_line_num,
            })
            .chain((0..after.len()).map(|hunk_line_num| HunkLineKey {
                file_num,
                hunk_num,
                hunk_type: HunkType::After,
                hunk_line_num,
            }));
        for hunk_line_key in hunk_line_keys {
            siv.call_on_name(&hunk_line_key.view_id(), |checkbox: &mut Checkbox| {
                checkbox.set_checked(new_value);
            });
        }

        self.refresh_file(siv, FileKey { file_num });
    }

    fn toggle_hunk_line(
        &mut self,
        siv: &mut CursiveRunner<CursiveRunnable>,
        hunk_line_key: HunkLineKey,
        new_value: bool,
    ) {
        let HunkLineKey {
            file_num,
            hunk_num,
            hunk_type,
            hunk_line_num,
        } = hunk_line_key;

        let (path, file_hunks) = &mut self.state.files[file_num];
        let FileHunks { hunks } = file_hunks;
        {
            let hunk = &mut hunks[hunk_num];
            let hunk_changed_lines = match (hunk, hunk_type) {
                (Hunk::Unchanged { contents }, _) => {
                    error!(?hunk_num, ?path, ?contents, "Invalid hunk num to change");
                    panic!("Invalid hunk num to change");
                }
                (Hunk::Changed { before, .. }, HunkType::Before) => before,
                (Hunk::Changed { after, .. }, HunkType::After) => after,
            };
            hunk_changed_lines[hunk_line_num].is_selected = new_value;
        }

        self.refresh_hunk(siv, HunkKey { file_num, hunk_num });
        self.refresh_file(siv, FileKey { file_num });
    }

    fn refresh_hunk(&mut self, siv: &mut CursiveRunner<CursiveRunnable>, hunk_key: HunkKey) {
        let HunkKey { file_num, hunk_num } = hunk_key;
        let hunk = &mut self.state.files[file_num].1.hunks[hunk_num];

        let hunk_selections = iter_hunk_changed_lines(hunk).map(
            |(
                _hunk_type,
                HunkChangedLine {
                    is_selected,
                    line: _,
                },
            )| *is_selected,
        );
        let hunk_new_value = all_are_same_value(hunk_selections).into_tristate();
        let hunk_key = HunkKey { file_num, hunk_num };
        siv.call_on_name(&hunk_key.view_id(), |tristate_box: &mut TristateBox| {
            tristate_box.set_state(hunk_new_value);
        });
    }

    fn refresh_file(&mut self, siv: &mut CursiveRunner<CursiveRunnable>, file_key: FileKey) {
        let FileKey { file_num } = file_key;
        let file_hunks = &mut self.state.files[file_num].1;

        let file_selections = iter_file_changed_lines(file_hunks).map(
            |(
                _hunk_type,
                HunkChangedLine {
                    is_selected,
                    line: _,
                },
            )| *is_selected,
        );
        let file_new_value = all_are_same_value(file_selections).into_tristate();
        siv.call_on_name(&file_key.view_id(), |tristate_box: &mut TristateBox| {
            tristate_box.set_state(file_new_value);
        });
    }
}

fn iter_file_changed_lines(
    hunks: &FileHunks,
) -> impl Iterator<Item = (HunkType, &HunkChangedLine)> {
    let FileHunks { hunks } = hunks;
    hunks.iter().flat_map(iter_hunk_changed_lines)
}

fn iter_hunk_changed_lines(hunk: &Hunk) -> impl Iterator<Item = (HunkType, &HunkChangedLine)> {
    let iter: Box<dyn Iterator<Item = (HunkType, &HunkChangedLine)>> = match hunk {
        Hunk::Changed { before, after } => Box::new(
            before
                .iter()
                .map(|changed_line| (HunkType::Before, changed_line))
                .chain(
                    after
                        .iter()
                        .map(|changed_line| (HunkType::After, changed_line)),
                ),
        ),
        Hunk::Unchanged { contents: _ } => Box::new(std::iter::empty()),
    };
    iter
}

#[derive(Clone, Debug)]
pub enum Message {
    Init,
    ToggleFile(FileKey, Tristate),
    ToggleHunk(HunkKey, Tristate),
    ToggleHunkLine(HunkLineKey, bool),
    Confirm,
    Quit,
}

impl EventDrivenCursiveApp for Recorder {
    type Message = Message;

    type Return = Result<RecordState, RecordError>;

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

            Message::ToggleHunk(hunk_key, new_value) => {
                self.toggle_hunk(siv, hunk_key, new_value);
            }

            Message::ToggleHunkLine(hunk_line_key, new_value) => {
                self.toggle_hunk_line(siv, hunk_line_key, new_value);
            }

            Message::Confirm => {
                self.did_user_confirm_exit = true;
                siv.quit();
            }

            Message::Quit => {
                let has_changes = {
                    let RecordState { files } = &self.state;
                    let changed_lines = files
                        .iter()
                        .flat_map(|(_, hunks)| iter_file_changed_lines(hunks))
                        .map(
                            |(
                                _hunk_type,
                                HunkChangedLine {
                                    is_selected,
                                    line: _,
                                },
                            )| is_selected,
                        );
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
    use std::convert::Infallible;
    use std::path::PathBuf;
    use std::rc::Rc;

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

    fn example_record_state() -> RecordState {
        RecordState {
            files: vec![(
                PathBuf::from("foo"),
                FileHunks {
                    hunks: vec![
                        Hunk::Unchanged {
                            contents: vec!["unchanged 1".to_string(), "unchanged 2".to_string()],
                        },
                        Hunk::Changed {
                            before: vec![
                                HunkChangedLine {
                                    is_selected: true,
                                    line: "before 1".to_string(),
                                },
                                HunkChangedLine {
                                    is_selected: true,
                                    line: "before 2".to_string(),
                                },
                            ],
                            after: vec![
                                HunkChangedLine {
                                    is_selected: true,
                                    line: "after 1".to_string(),
                                },
                                HunkChangedLine {
                                    is_selected: false,
                                    line: "after 2".to_string(),
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
        [~] /foo
        1 unchanged 1
        2 unchanged 2
        [~] hunk 1/1 in current file, 1/1 total
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
                CursiveTestingEvent::Event(Key::Down.into()), // hunk
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
        [X] /foo
        1 unchanged 1
        2 unchanged 2
        [X] hunk 1/1 in current file, 1/1 total
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
    fn test_hunk_toggle() {
        let screenshot1 = Default::default();
        let screenshot2 = Default::default();
        let screenshot3 = Default::default();
        let screenshot4 = Default::default();
        let result = run_test(
            example_record_state(),
            vec![
                CursiveTestingEvent::Event(Key::Down.into()), // move to hunk
                CursiveTestingEvent::Event(' '.into()),       // toggle hunk
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
                CursiveTestingEvent::Event(' '.into()), // toggle hunk
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot2)),
                CursiveTestingEvent::Event(Key::Down.into()), // move to line
                CursiveTestingEvent::Event(' '.into()),       // toggle line
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot3)),
                CursiveTestingEvent::Event(Key::Up.into()), // move to hunk
                CursiveTestingEvent::Event(' '.into()),     // toggle hunk
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot4)),
                CursiveTestingEvent::Event('c'.into()),
            ],
        );

        insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
        [X] /foo
        1 unchanged 1
        2 unchanged 2
        [X] hunk 1/1 in current file, 1/1 total
        [X] -before 1
        [X] -before 2
        [X] +after 1
        [X] +after 2
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot2), @r###"
        [ ] /foo
        1 unchanged 1
        2 unchanged 2
        [ ] hunk 1/1 in current file, 1/1 total
        [ ] -before 1
        [ ] -before 2
        [ ] +after 1
        [ ] +after 2
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot3), @r###"
        [~] /foo
        1 unchanged 1
        2 unchanged 2
        [~] hunk 1/1 in current file, 1/1 total
        [X] -before 1
        [ ] -before 2
        [ ] +after 1
        [ ] +after 2
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot4), @r###"
        [X] /foo
        1 unchanged 1
        2 unchanged 2
        [X] hunk 1/1 in current file, 1/1 total
        [X] -before 1
        [X] -before 2
        [X] +after 1
        [X] +after 2
        "###);
        insta::assert_debug_snapshot!(result, @r###"
        Ok(
            RecordState {
                files: [
                    (
                        "foo",
                        FileHunks {
                            hunks: [
                                Unchanged {
                                    contents: [
                                        "unchanged 1",
                                        "unchanged 2",
                                    ],
                                },
                                Changed {
                                    before: [
                                        HunkChangedLine {
                                            is_selected: true,
                                            line: "before 1",
                                        },
                                        HunkChangedLine {
                                            is_selected: true,
                                            line: "before 2",
                                        },
                                    ],
                                    after: [
                                        HunkChangedLine {
                                            is_selected: true,
                                            line: "after 1",
                                        },
                                        HunkChangedLine {
                                            is_selected: true,
                                            line: "after 2",
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
                CursiveTestingEvent::Event(Key::Down.into()), // move to hunk
                CursiveTestingEvent::Event(' '.into()),       // toggle hunk
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
        [X] /foo
        1 unchanged 1
        2 unchanged 2
        [X] hunk 1/1 in current file, 1/1 total
        [X] -before 1
        [X] -before 2
        [X] +after 1
        [X] +after 2
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot2), @r###"
        [ ] /foo
        1 unchanged 1
        2 unchanged 2
        [ ] hunk 1/1 in current file, 1/1 total
        [ ] -before 1
        [ ] -before 2
        [ ] +after 1
        [ ] +after 2
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot3), @r###"
        [~] /foo
        1 unchanged 1
        2 unchanged 2
        [~] hunk 1/1 in current file, 1/1 total
        [X] -before 1
        [ ] -before 2
        [ ] +after 1
        [ ] +after 2
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot4), @r###"
        [X] /foo
        1 unchanged 1
        2 unchanged 2
        [X] hunk 1/1 in current file, 1/1 total
        [X] -before 1
        [X] -before 2
        [X] +after 1
        [X] +after 2
        "###);
        insta::assert_debug_snapshot!(result, @r###"
        Ok(
            RecordState {
                files: [
                    (
                        "foo",
                        FileHunks {
                            hunks: [
                                Unchanged {
                                    contents: [
                                        "unchanged 1",
                                        "unchanged 2",
                                    ],
                                },
                                Changed {
                                    before: [
                                        HunkChangedLine {
                                            is_selected: true,
                                            line: "before 1",
                                        },
                                        HunkChangedLine {
                                            is_selected: true,
                                            line: "before 2",
                                        },
                                    ],
                                    after: [
                                        HunkChangedLine {
                                            is_selected: true,
                                            line: "after 1",
                                        },
                                        HunkChangedLine {
                                            is_selected: true,
                                            line: "after 2",
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
            let (_path, FileHunks { hunks }) = &mut state.files[0];
            for hunk in hunks {
                if let Hunk::Changed { before, after: _ } = hunk {
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
        [~] /foo
        1 unchanged 1
        2 unchanged 2
        [~] hunk 1/1 in current file, 1/1 total
        [X] -before 1
        [X] -before 2
        [X] +after 1
        [ ] +after 2
        "###);
    }
}
