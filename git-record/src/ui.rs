use std::fmt::Display;
use std::sync::mpsc::Sender;

use cursive::event::Event;
use cursive::theme::{BaseColor, Effect};
use cursive::traits::{Nameable, Resizable};
use cursive::utils::markup::StyledString;
use cursive::views::{Checkbox, Dialog, HideableView, LinearLayout, ScrollView, TextView};
use cursive::{CursiveRunnable, CursiveRunner, View};
use tracing::error;

use crate::cursive_utils::EventDrivenCursiveApp;
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

impl Display for FileKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { file_num } = self;
        write!(f, "FileKey({})", file_num)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct HunkKey {
    file_num: usize,
    hunk_num: usize,
}

impl Display for HunkKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { file_num, hunk_num } = self;
        write!(f, "HunkKey({},{})", *file_num, *hunk_num)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct HunkLineKey {
    file_num: usize,
    hunk_num: usize,
    hunk_type: HunkType,
    hunk_line_num: usize,
}

impl Display for HunkLineKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            file_num,
            hunk_num,
            hunk_type,
            hunk_line_num,
        } = self;
        write!(
            f,
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

pub struct Recorder<'a> {
    did_user_confirm_exit: bool,
    state: RecordState<'a>,
}

impl<'a> Recorder<'a> {
    pub fn new(state: RecordState<'a>) -> Self {
        Self {
            did_user_confirm_exit: false,
            state,
        }
    }

    fn make_view(&self, main_tx: Sender<Message>) -> impl View {
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
                        for line in contents.lines() {
                            line_num += 1;
                            file_view.add_child(TextView::new(format!("  {} {}", line_num, line)));
                        }
                    }

                    Hunk::Changed {
                        before,
                        after,
                        parent,
                    } => {
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
                            parent,
                        );
                    }
                }
            }

            view.add_child(
                LinearLayout::horizontal()
                    .child(
                        TristateBox::new()
                            .with_state({
                                match all_are_same_value(iter_file_changed_lines(file_hunks).map(
                                    |(
                                        _hunk_type,
                                        HunkChangedLine {
                                            is_selected,
                                            line: _,
                                        },
                                    )| is_selected,
                                )) {
                                    SameValueResult::Empty | SameValueResult::AllSame(true) => {
                                        Tristate::Checked
                                    }
                                    SameValueResult::AllSame(false) => Tristate::Unchecked,
                                    SameValueResult::SomeDifferent => Tristate::Partial,
                                }
                            })
                            .on_change({
                                let main_tx = main_tx.clone();
                                move |_, new_value| {
                                    let file_key = FileKey { file_num };
                                    if main_tx
                                        .send(Message::ToggleFile(file_key, new_value))
                                        .is_err()
                                    {
                                        // Do nothing.
                                    }
                                }
                            })
                            .with_name(file_key.to_string()),
                    )
                    .child(TextView::new({
                        let mut s = StyledString::new();
                        s.append_plain(" /");
                        s.append_styled(path.to_string_lossy(), Effect::Bold);
                        s
                    })),
            );
            view.add_child(file_view);
            if file_num + 1 < files.len() {
                // Render a spacer line. Note that an empty string won't render an
                // empty line.
                view.add_child(TextView::new(" "));
            }
        }

        view
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
        parent: &Option<&str>,
    ) {
        let mut hunk_view = LinearLayout::vertical();
        let hunk_key = HunkKey { file_num, hunk_num };

        for (hunk_line_num, HunkChangedLine { is_selected, line }) in before.iter().enumerate() {
            *line_num += 1;
            let line_contents = {
                let mut s = StyledString::new();
                s.append_styled(format!(" -{}", line), BaseColor::Red.dark());
                s
            };
            let hunk_line_key = HunkLineKey {
                file_num,
                hunk_num,
                hunk_type: HunkType::Before,
                hunk_line_num,
            };
            hunk_view.add_child(
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
                            .with_name(hunk_line_key.to_string()),
                    )
                    .child(TextView::new(line_contents)),
            );
        }

        if let Some(parent) = parent {
            for line in parent.lines() {
                hunk_view.add_child(TextView::new(format!("         {}", line)));
            }
        }

        for (hunk_line_num, HunkChangedLine { is_selected, line }) in after.iter().enumerate() {
            let line_contents = {
                let mut s = StyledString::new();
                s.append_styled(format!(" +{}", line), BaseColor::Green.dark());
                s
            };
            let hunk_line_key = HunkLineKey {
                file_num,
                hunk_num,
                hunk_type: HunkType::After,
                hunk_line_num,
            };
            hunk_view.add_child(
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
                            .with_name(hunk_line_key.to_string()),
                    )
                    .child(TextView::new(line_contents)),
            );
        }

        view.add_child(
            LinearLayout::horizontal()
                .child(TextView::new("  "))
                .child(
                    TristateBox::new()
                        // TODO: set this initial state correctly
                        .with_state(Tristate::Checked)
                        .on_change({
                            let hunk_key = HunkKey { file_num, hunk_num };
                            let main_tx = main_tx.clone();
                            move |_, new_value| {
                                if main_tx
                                    .send(Message::ToggleHunk(hunk_key, new_value))
                                    .is_err()
                                {
                                    // Do nothing.
                                }
                            }
                        })
                        .with_name(hunk_key.to_string()),
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
                Hunk::Changed {
                    before,
                    after,
                    parent: _,
                } => (before, after),
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
            siv.call_on_name(&hunk_line_key.to_string(), |checkbox: &mut Checkbox| {
                checkbox.set_checked(new_value);
            });
        }
    }
}

fn iter_file_changed_lines<'a>(
    hunks: &'a FileHunks<'a>,
) -> impl Iterator<Item = (HunkType, &'a HunkChangedLine<'a>)> {
    let FileHunks { hunks } = hunks;
    hunks.iter().flat_map(iter_hunk_changed_lines)
}

fn iter_hunk_changed_lines<'a>(
    hunk: &'a Hunk<'a>,
) -> impl Iterator<Item = (HunkType, &'a HunkChangedLine<'a>)> {
    let iter: Box<dyn Iterator<Item = (HunkType, &HunkChangedLine)>> = match hunk {
        Hunk::Changed {
            before,
            after,
            parent: _,
        } => Box::new(
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
                let view = self.make_view(main_tx.clone());
                siv.add_layer(ScrollView::new(
                    // NB: you can't add `min_width` to the `ScrollView` itself,
                    // or else the scrollbar stops responding to clicks and
                    // drags.
                    view.min_width(80),
                ));
            }

            Message::ToggleFile(file_key, new_value) => {
                let new_value = match new_value {
                    Tristate::Unchecked => false,
                    Tristate::Partial => {
                        // Shouldn't happen.
                        true
                    }
                    Tristate::Checked => true,
                };

                // iter_file_changed_lines(hunks)
            }

            Message::ToggleHunk(hunk_key, new_value) => {
                self.toggle_hunk(siv, hunk_key, new_value);
            }

            Message::ToggleHunkLine(
                HunkLineKey {
                    file_num,
                    hunk_num,
                    hunk_type,
                    hunk_line_num,
                },
                new_value,
            ) => {
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

                // Refresh parent hunk checkbox.
                let hunk = &mut hunks[hunk_num];
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
                siv.call_on_name(&hunk_key.to_string(), |tristate_box: &mut TristateBox| {
                    tristate_box.set_state(hunk_new_value);
                });

                // Refresh parent file checkbox.
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
                let file_key = FileKey { file_num };
                siv.call_on_name(&file_key.to_string(), |tristate_box: &mut TristateBox| {
                    tristate_box.set_state(file_new_value);
                });
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
    use crate::cursive_utils::EventDrivenCursiveAppExt;

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
            files: vec![(
                PathBuf::from("foo"),
                FileHunks {
                    hunks: vec![
                        Hunk::Unchanged {
                            contents: "unchanged 1\nunchanged 2\n",
                        },
                        Hunk::Changed {
                            before: vec![
                                HunkChangedLine {
                                    is_selected: true,
                                    line: "before 1",
                                },
                                HunkChangedLine {
                                    is_selected: true,
                                    line: "before 2",
                                },
                            ],
                            after: vec![
                                HunkChangedLine {
                                    is_selected: true,
                                    line: "after 1",
                                },
                                HunkChangedLine {
                                    is_selected: false,
                                    line: "after 2",
                                },
                            ],
                            parent: Some("parent 1\nparent 2\n"),
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
        [X] hunk 1/1 in current file, 1/1 total
        [X] -before 1
        [X] -before 2
        parent 1
        parent 2
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
        parent 1
        parent 2
        [X] +after 1
        [X] +after 2
        "###);
        insta::assert_debug_snapshot!(result, @r###"
        Err(
            Cancelled,
        )
        "###);
    }
}
