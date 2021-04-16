//! Allows undoing to a previous state of the repo.
//!
//! This is accomplished by finding the events that have happened since a certain
//! time and inverting them.

use std::convert::TryInto;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};

use std::time::SystemTime;

use anyhow::Context;

use cursive::event::Key;
use cursive::theme::{Color, PaletteColor};
use cursive::traits::Nameable;
use cursive::views::{
    Dialog, EditView, LinearLayout, NamedView, OnEventView, ScrollView, TextView,
};
use cursive::{Cursive, CursiveRunnable, CursiveRunner};

use crate::commands::smartlog::render_graph;
use crate::core::eventlog::{Event, EventCursor, EventLogDb, EventReplayer, EventTransactionId};
use crate::core::formatting::{Glyphs, Pluralize};
use crate::core::graph::{make_graph, BranchOids, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::core::metadata::{
    render_commit_metadata, BranchesProvider, CommitMessageProvider, CommitOidProvider,
    DifferentialRevisionProvider, RelativeTimeProvider,
};
use crate::util::{get_db_conn, get_repo, run_git, GitExecutable};

pub(crate) fn with_siv<T, F: FnOnce(CursiveRunner<CursiveRunnable>) -> anyhow::Result<T>>(
    f: F,
) -> anyhow::Result<T> {
    // I tried these back-ends:
    //
    // * `ncurses`/`pancurses`: Doesn't render ANSI escape codes. (NB: the fact
    //   that we print out strings with ANSI escape codes is tech debt; we would
    //   ideally pass around styled representations of all text, and decide how to
    //   rendering it later.) Rendered scroll view improperly. No mouse/scrolling
    //   support
    // * `termion`: Renders ANSI escape codes. Has mouse/scrolling support. But
    //   critical bug: https://github.com/gyscos/cursive/issues/563
    // * `crossterm`: Renders ANSI escape codes. Has mouse/scrolling support.
    //   However, has some flickering issues, particularly when scrolling. See
    //   issue at https://github.com/gyscos/cursive/issues/142. I tried the
    //   `cursive_buffered_backend` library, but this causes it to no longer
    //   respect the ANSI escape codes.
    // * `blt`: Seems to require that a certain library be present on the system
    //   for linking.
    let mut siv = cursive::crossterm();

    siv.update_theme(|theme| {
        theme.shadow = false;
        theme.palette.extend(vec![
            (PaletteColor::Background, Color::TerminalDefault),
            (PaletteColor::View, Color::TerminalDefault),
            (PaletteColor::Primary, Color::TerminalDefault),
        ]);
    });
    let old_max_level = log::max_level();
    log::set_max_level(log::LevelFilter::Off);
    let result = f(siv.into_runner());
    log::set_max_level(old_max_level);
    let result = result?;
    Ok(result)
}

fn render_cursor_smartlog(
    glyphs: &Glyphs,
    repo: &git2::Repository,
    merge_base_db: &MergeBaseDb,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
) -> anyhow::Result<String> {
    let head_oid = event_replayer.get_cursor_head_oid(event_cursor);
    let main_branch_oid = event_replayer.get_cursor_main_branch_oid(event_cursor, repo)?;
    let branch_oid_to_names = event_replayer.get_cursor_branch_oid_to_names(event_cursor, repo)?;
    let graph = make_graph(
        repo,
        merge_base_db,
        event_replayer,
        event_cursor,
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        true,
    )?;
    let mut out = Vec::new();
    render_graph(
        &mut out,
        glyphs,
        repo,
        merge_base_db,
        &graph,
        &HeadOid(head_oid),
        &[
            &CommitOidProvider::new(true)?,
            &RelativeTimeProvider::new(&repo, SystemTime::now())?,
            &BranchesProvider::new(&repo, &branch_oid_to_names)?,
            &DifferentialRevisionProvider::new(&repo)?,
            &CommitMessageProvider::new()?,
        ],
    )?;
    let result = String::from_utf8(out)?;
    Ok(result)
}

fn render_ref_name(ref_name: &str) -> String {
    match ref_name.strip_prefix("refs/heads/") {
        Some(branch_name) => format!("branch {}", branch_name),
        None => format!("ref {}", ref_name),
    }
}

fn describe_event(repo: &git2::Repository, event: &Event) -> anyhow::Result<String> {
    let render_commit = |oid: git2::Oid| -> anyhow::Result<String> {
        match repo.find_commit(oid) {
            Ok(commit) => render_commit_metadata(
                &commit,
                &[
                    &CommitOidProvider::new(true)?,
                    &CommitMessageProvider::new()?,
                ],
            ),
            Err(_) => Ok(format!(
                "<unavailable: {} (possibly GC'ed)>",
                oid.to_string()
            )),
        }
    };
    let result = match event {
        Event::CommitEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid,
        } => {
            format!("Commit {}\n", render_commit(*commit_oid)?)
        }

        Event::HideEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid,
        } => {
            format!("Hide commit {}\n", render_commit(*commit_oid)?)
        }

        Event::UnhideEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid,
        } => {
            format!("Unhide commit {}\n", render_commit(*commit_oid)?)
        }

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_ref: None,
            new_ref: Some(new_ref),
            message: _,
        } if ref_name == "HEAD" => {
            // Not sure if this can happen. When a repo is created, maybe?
            format!("Check out to {}\n", render_commit(new_ref.parse()?)?)
        }

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_ref: Some(old_ref),
            new_ref: Some(new_ref),
            message: _,
        } if ref_name == "HEAD" => {
            format!(
                "\
Check out from {}
            to {}",
                render_commit(old_ref.parse()?)?,
                render_commit(new_ref.parse()?)?
            )
        }

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_ref: None,
            new_ref: None,
            message: _,
        } => {
            format!(
                "\
Empty event for {}
This event should not appear. This is a (benign) bug -- please report it.
",
                render_ref_name(ref_name)
            )
        }

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_ref: None,
            new_ref: Some(new_ref),
            message: _,
        } => {
            format!(
                "Create {} at {}\n",
                render_ref_name(ref_name),
                render_commit(new_ref.parse()?)?
            )
        }

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_ref: Some(old_ref),
            new_ref: None,
            message: _,
        } => {
            format!(
                "Delete {} at {}\n",
                render_ref_name(ref_name),
                render_commit(old_ref.parse()?)?
            )
        }

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_ref: Some(old_ref),
            new_ref: Some(new_ref),
            message: _,
        } => {
            let ref_name = render_ref_name(ref_name);
            format!(
                "\
Move {} from {}
     {}   to {}",
                ref_name,
                render_commit(old_ref.parse()?)?,
                " ".repeat(ref_name.len()),
                render_commit(new_ref.parse()?)?,
            )
        }

        Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid,
            new_commit_oid,
        } => {
            format!(
                "\
Rewrite commit {}
            as {}",
                render_commit(*old_commit_oid)?,
                render_commit(*new_commit_oid)?
            )
        }
    };
    Ok(result)
}

fn describe_events_numbered(
    out: &mut impl Write,
    repo: &git2::Repository,
    events: &[Event],
) -> Result<(), anyhow::Error> {
    for (i, event) in (1..).zip(events) {
        let num_header = format!("{}. ", i);
        for (j, line) in (0..).zip(describe_event(&repo, &event)?.split('\n')) {
            if j == 0 {
                write!(out, "{}", num_header)?;
            } else {
                write!(out, "{}", " ".repeat(num_header.len()))?;
            }
            writeln!(out, "{}", line)?;
        }
    }
    Ok(())
}

fn select_past_event(
    mut siv: CursiveRunner<CursiveRunnable>,
    glyphs: &Glyphs,
    repo: &git2::Repository,
    merge_base_db: &MergeBaseDb,
    event_replayer: &mut EventReplayer,
) -> anyhow::Result<Option<EventCursor>> {
    #[derive(Clone, Copy, Debug)]
    enum Message {
        Init,
        Next,
        Previous,
        GoToEvent,
        SetEventReplayerCursor { event_id: isize },
        Help,
        Quit,
        SelectEventIdAndQuit,
    }
    let (main_tx, main_rx): (Sender<Message>, Receiver<Message>) = channel();

    [
        ('n'.into(), Message::Next),
        ('N'.into(), Message::Next),
        (Key::Right.into(), Message::Next),
        ('p'.into(), Message::Previous),
        ('P'.into(), Message::Previous),
        (Key::Left.into(), Message::Previous),
        ('h'.into(), Message::Help),
        ('H'.into(), Message::Help),
        ('?'.into(), Message::Help),
        ('g'.into(), Message::GoToEvent),
        ('G'.into(), Message::GoToEvent),
        ('q'.into(), Message::Quit),
        ('Q'.into(), Message::Quit),
        (
            cursive::event::Key::Enter.into(),
            Message::SelectEventIdAndQuit,
        ),
    ]
    .iter()
    .cloned()
    .for_each(|(event, message): (cursive::event::Event, Message)| {
        siv.add_global_callback(event, {
            let main_tx = main_tx.clone();
            move |_siv| main_tx.send(message).unwrap()
        });
    });

    let mut cursor = event_replayer.make_default_cursor();
    let now = SystemTime::now();
    main_tx.send(Message::Init)?;
    while siv.is_running() {
        let message = main_rx.try_recv();
        if message.is_err() {
            // For tests: only pump the Cursive event loop if we have no events
            // of our own to process. Otherwise, the event loop queues up all of
            // the messages before we can process them, which means that none of
            // the screenshots are correct.
            siv.step();
        }

        type SmartlogView = ScrollView<TextView>;
        const SMARTLOG_VIEW_NAME: &str = "smartlog-view";
        type InfoView = TextView;
        const INFO_VIEW_NAME: &str = "info-view";
        let redraw = |siv: &mut Cursive,
                      event_replayer: &mut EventReplayer,
                      event_cursor: EventCursor|
         -> anyhow::Result<()> {
            let smartlog = render_cursor_smartlog(
                &glyphs,
                &repo,
                &merge_base_db,
                &event_replayer,
                event_cursor,
            )?;
            siv.find_name::<SmartlogView>(SMARTLOG_VIEW_NAME)
                .unwrap()
                .get_inner_mut()
                .set_content(smartlog);

            let event = event_replayer.get_tx_events_before_cursor(event_cursor);
            let info_view_contents = match event {
                None => "There are no previous available events.".to_owned(),
                Some((event_id, events)) => {
                    let event_description = {
                        let mut out = Vec::new();
                        describe_events_numbered(&mut out, repo, &events)?;
                        String::from_utf8(out)?
                    };
                    let relative_time_provider = RelativeTimeProvider::new(repo, now)?;
                    let relative_time = if relative_time_provider.is_enabled() {
                        format!(
                            " ({} ago)",
                            RelativeTimeProvider::describe_time_delta(
                                now,
                                events[0].get_timestamp()
                            )?
                        )
                    } else {
                        String::new()
                    };
                    format!(
                            "Repo after transaction {event_tx_id} (event {event_id}){relative_time}. Press 'h' for help, 'q' to quit.\n{event_description}\n",
                            event_tx_id = events[0].get_event_tx_id().to_string(),
                            event_id = event_id,
                            relative_time = relative_time,
                            event_description = event_description,
                        )
                }
            };
            siv.find_name::<InfoView>(INFO_VIEW_NAME)
                .unwrap()
                .set_content(info_view_contents);
            Ok(())
        };

        match message {
            Err(TryRecvError::Disconnected) => break,

            Err(TryRecvError::Empty) => {
                // If we haven't received a message yet, defer to `siv.step`
                // to process the next user input.
                continue;
            }

            Ok(Message::Init) => {
                let smartlog_view: NamedView<SmartlogView> =
                    ScrollView::new(TextView::new("")).with_name(SMARTLOG_VIEW_NAME);
                let info_view: NamedView<InfoView> = TextView::new("").with_name(INFO_VIEW_NAME);
                siv.add_layer(
                    LinearLayout::vertical()
                        .child(smartlog_view)
                        .child(info_view),
                );
                redraw(&mut siv, event_replayer, cursor)?;
            }

            Ok(Message::Next) => {
                cursor = event_replayer.advance_cursor_by_transaction(cursor, 1);
                redraw(&mut siv, event_replayer, cursor)?;
            }

            Ok(Message::Previous) => {
                cursor = event_replayer.advance_cursor_by_transaction(cursor, -1);
                redraw(&mut siv, event_replayer, cursor)?;
            }

            Ok(Message::SetEventReplayerCursor { event_id }) => {
                cursor = event_replayer.make_cursor(event_id);
                redraw(&mut siv, event_replayer, cursor)?;
            }

            Ok(Message::GoToEvent) => {
                let main_tx = main_tx.clone();
                siv.add_layer(
                    OnEventView::new(
                        Dialog::new()
                            .title("Go to event")
                            .content(EditView::new().on_submit(move |siv, text| {
                                match text.parse::<isize>() {
                                    Ok(event_id) => {
                                        main_tx
                                            .send(Message::SetEventReplayerCursor { event_id })
                                            .unwrap();
                                        siv.pop_layer();
                                    }
                                    Err(_) => {
                                        siv.add_layer(Dialog::info(format!(
                                            "Invalid event ID: {}",
                                            text
                                        )));
                                    }
                                }
                            }))
                            .dismiss_button("Cancel"),
                    )
                    .on_event(Key::Esc, |siv| {
                        siv.pop_layer();
                    }),
                );
            }

            Ok(Message::Help) => {
                siv.add_layer(
                        Dialog::new()
                            .title("How to use")
                            .content(TextView::new(
"Use `git undo` to view and revert to previous states of the repository.

h/?: Show this help.
q: Quit.
p/n or <left>/<right>: View next/previous state.
g: Go to a provided event ID.
<enter>: Revert the repository to the given state (requires confirmation).

You can also copy a commit hash from the past and manually run `git unhide` or `git rebase` on it.
",
                            ))
                            .dismiss_button("Close"),
                    );
            }

            Ok(Message::Quit) => siv.quit(),

            Ok(Message::SelectEventIdAndQuit) => {
                siv.quit();
                match event_replayer.get_event_before_cursor(cursor) {
                    Some(_) => return Ok(Some(cursor)),
                    None => return Ok(None),
                }
            }
        };

        if message.is_ok() {
            siv.refresh();
        }
    }

    Ok(None)
}

fn inverse_event(
    event: Event,
    now: SystemTime,
    event_tx_id: EventTransactionId,
) -> anyhow::Result<Event> {
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();
    let inverse_event = match event {
        Event::CommitEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid,
        }
        | Event::UnhideEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid,
        } => Event::HideEvent {
            timestamp,
            event_tx_id,
            commit_oid,
        },

        Event::HideEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid,
        } => Event::UnhideEvent {
            timestamp,
            event_tx_id,
            commit_oid,
        },

        Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid,
            new_commit_oid,
        } => Event::RewriteEvent {
            timestamp,
            event_tx_id,
            old_commit_oid: new_commit_oid,
            new_commit_oid: old_commit_oid,
        },

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_ref,
            new_ref,
            message: _,
        } => Event::RefUpdateEvent {
            timestamp,
            event_tx_id,
            ref_name,
            old_ref: new_ref,
            new_ref: old_ref,
            message: None,
        },
    };
    Ok(inverse_event)
}

fn optimize_inverse_events(events: Vec<Event>) -> Vec<Event> {
    let mut optimized_events = Vec::new();
    let mut seen_checkout = false;
    for event in events.into_iter().rev() {
        match event {
            Event::RefUpdateEvent { ref ref_name, .. } if ref_name == "HEAD" => {
                if seen_checkout {
                    continue;
                } else {
                    seen_checkout = true;
                    optimized_events.push(event)
                }
            }
            event => optimized_events.push(event),
        };
    }
    optimized_events.reverse();
    optimized_events
}

fn undo_events(
    in_: &mut impl Read,
    out: &mut impl Write,
    err: &mut impl Write,
    repo: &git2::Repository,
    git_executable: &GitExecutable,
    event_log_db: &mut EventLogDb,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
) -> anyhow::Result<isize> {
    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "undo")?;
    let inverse_events: Vec<Event> = event_replayer
        .get_events_since_cursor(event_cursor)
        .iter()
        .rev()
        .filter(|event| {
            !matches!(
                event,
                Event::RefUpdateEvent {
                    timestamp: _,
                    event_tx_id: _,
                    ref_name,
                    old_ref: None,
                    new_ref: _,
                    message: _,
                } if ref_name == "HEAD"
            )
        })
        .map(|event| inverse_event(event.clone(), now, event_tx_id))
        .collect::<anyhow::Result<Vec<Event>>>()?;
    let inverse_events = optimize_inverse_events(inverse_events);
    if inverse_events.is_empty() {
        writeln!(out, "No undo actions to apply, exiting.")?;
        return Ok(0);
    }

    writeln!(out, "Will apply these actions:")?;
    describe_events_numbered(out, repo, &inverse_events)?;

    let confirmed = {
        write!(out, "Confirm? [yN] ")?;
        out.flush()?;
        let mut user_input = String::new();
        let mut reader = BufReader::new(in_);
        match reader.read_line(&mut user_input) {
            Ok(_size) => {
                let user_input = user_input.trim();
                user_input == "y" || user_input == "Y"
            }
            Err(_) => false,
        }
    };
    if !confirmed {
        writeln!(out, "Aborted.")?;
        return Ok(1);
    }

    let num_inverse_events = Pluralize {
        amount: inverse_events.len().try_into().unwrap(),
        singular: "inverse event",
        plural: "inverse events",
    }
    .to_string();
    for event in inverse_events.into_iter() {
        match event {
            Event::RefUpdateEvent {
                timestamp: _,
                event_tx_id: _,
                ref_name,
                old_ref: _,
                new_ref: Some(new_ref),
                message: _,
            } if ref_name == "HEAD" => {
                // Most likely the user wanted to perform an actual checkout in
                // this case, rather than just update `HEAD` (and be left with a
                // dirty working copy). The `Git` command will update the event
                // log appropriately, as it will invoke our hooks.
                run_git(
                    out,
                    err,
                    git_executable,
                    Some(event_tx_id),
                    &["checkout", &new_ref],
                )
                .with_context(|| "Updating to previous HEAD location")?;
            }
            Event::RefUpdateEvent {
                timestamp: _,
                event_tx_id: _,
                ref_name: _,
                old_ref: None,
                new_ref: None,
                message: _,
            } => {
                // Do nothing.
            }
            Event::RefUpdateEvent {
                timestamp: _,
                event_tx_id: _,
                ref_name,
                old_ref: Some(_),
                new_ref: None,
                message: _,
            } => match repo.find_reference(&ref_name) {
                Ok(mut reference) => {
                    reference
                        .delete()
                        .with_context(|| format!("Deleting reference: {}", ref_name))?;
                }
                Err(_) => {
                    writeln!(
                        out,
                        "Reference {} did not exist, not deleting it.",
                        ref_name
                    )?;
                }
            },
            Event::RefUpdateEvent {
                timestamp: _,
                event_tx_id: _,
                ref_name,
                old_ref: None,
                new_ref: Some(new_ref),
                message: _,
            }
            | Event::RefUpdateEvent {
                timestamp: _,
                event_tx_id: _,
                ref_name,
                old_ref: Some(_),
                new_ref: Some(new_ref),
                message: _,
            } => {
                // Create or update the given reference.
                let new_ref = new_ref.parse()?;
                repo.reference(&ref_name, new_ref, true, "branchless undo")?;
            }
            Event::CommitEvent { .. }
            | Event::HideEvent { .. }
            | Event::UnhideEvent { .. }
            | Event::RewriteEvent { .. } => {
                event_log_db.add_events(vec![event])?;
            }
        }
    }

    writeln!(out, "Applied {}.", num_inverse_events)?;
    Ok(0)
}

/// Restore the repository to a previous state interactively.
pub fn undo(
    in_: &mut impl Read,
    out: &mut impl Write,
    err: &mut impl Write,
    git_executable: &GitExecutable,
) -> anyhow::Result<isize> {
    let glyphs = Glyphs::detect();
    let repo = get_repo()?;
    let conn = get_db_conn(&repo)?;
    let merge_base_db = MergeBaseDb::new(&conn)?;
    let mut event_log_db = EventLogDb::new(&conn)?;
    let mut event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;

    let event_cursor = {
        let result = with_siv(|siv| {
            select_past_event(siv, &glyphs, &repo, &merge_base_db, &mut event_replayer)
        })?;
        match result {
            Some(event_cursor) => event_cursor,
            None => return Ok(0),
        }
    };

    let result = undo_events(
        in_,
        out,
        err,
        &repo,
        &git_executable,
        &mut event_log_db,
        &event_replayer,
        event_cursor,
    )?;
    Ok(result)
}

#[allow(missing_docs)]
pub mod testing {
    use std::io::{Read, Write};

    use cursive::{CursiveRunnable, CursiveRunner};

    use crate::core::eventlog::{EventCursor, EventLogDb, EventReplayer};
    use crate::core::formatting::Glyphs;
    use crate::core::mergebase::MergeBaseDb;
    use crate::util::GitExecutable;

    pub fn with_siv<T, F: FnOnce(CursiveRunner<CursiveRunnable>) -> anyhow::Result<T>>(
        f: F,
    ) -> anyhow::Result<T> {
        super::with_siv(f)
    }

    pub fn select_past_event(
        siv: CursiveRunner<CursiveRunnable>,
        glyphs: &Glyphs,
        repo: &git2::Repository,
        merge_base_db: &MergeBaseDb,
        event_replayer: &mut EventReplayer,
    ) -> anyhow::Result<Option<EventCursor>> {
        super::select_past_event(siv, glyphs, repo, merge_base_db, event_replayer)
    }

    pub fn undo_events(
        in_: &mut impl Read,
        out: &mut impl Write,
        err: &mut impl Write,
        repo: &git2::Repository,
        git_executable: &GitExecutable,
        event_log_db: &mut EventLogDb,
        event_replayer: &EventReplayer,
        event_cursor: EventCursor,
    ) -> anyhow::Result<isize> {
        super::undo_events(
            in_,
            out,
            err,
            repo,
            git_executable,
            event_log_db,
            event_replayer,
            event_cursor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::core::eventlog::testing::make_dummy_transaction_id;

    #[test]
    fn test_optimize_inverse_events() -> anyhow::Result<()> {
        let event_tx_id = make_dummy_transaction_id(123);
        let input = vec![
            Event::RefUpdateEvent {
                timestamp: 1.0,
                event_tx_id,
                ref_name: "HEAD".to_owned(),
                old_ref: Some("1".parse()?),
                new_ref: Some("2".parse()?),
                message: None,
            },
            Event::RefUpdateEvent {
                timestamp: 2.0,
                event_tx_id,
                ref_name: "HEAD".to_owned(),
                old_ref: Some("1".parse()?),
                new_ref: Some("3".parse()?),
                message: None,
            },
        ];
        let expected = vec![Event::RefUpdateEvent {
            timestamp: 2.0,
            event_tx_id,
            ref_name: "HEAD".to_owned(),
            old_ref: Some("1".parse()?),
            new_ref: Some("3".parse()?),
            message: None,
        }];
        assert_eq!(optimize_inverse_events(input), expected);
        Ok(())
    }
}
