//! Allows undoing to a previous state of the repo.
//!
//! This is accomplished by finding the events that have happened since a certain
//! time and inverting them.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_conditions)]

pub mod tui;

use std::fmt::Write;
use std::io::{stdin, BufRead, BufReader, Read};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::time::SystemTime;

use cursive_core::event::Key;
use cursive_core::traits::Resizable;
use cursive_core::utils::markup::StyledString;
use cursive_core::views::{
    Dialog, EditView, LinearLayout, OnEventView, Panel, ScrollView, TextView,
};
use cursive_core::{Cursive, CursiveRunner};
use eyre::Context;
use lib::core::check_out::{check_out_commit, CheckOutCommitOptions, CheckoutTarget};
use lib::core::repo_ext::RepoExt;
use lib::try_exit_code;
use lib::util::{ExitCode, EyreExitOr};
use tracing::instrument;

use crate::tui::{with_siv, SingletonView};
use git_branchless_revset::resolve_default_smartlog_commits;
use git_branchless_smartlog::{make_smartlog_graph, render_graph};
use lib::core::dag::{CommitSet, Dag};
use lib::core::effects::Effects;
use lib::core::eventlog::{Event, EventCursor, EventLogDb, EventReplayer, EventTransactionId};
use lib::core::formatting::{Glyphs, Pluralize, StyledStringBuilder};
use lib::core::node_descriptors::{
    BranchesDescriptor, CommitMessageDescriptor, CommitOidDescriptor,
    DifferentialRevisionDescriptor, ObsolescenceExplanationDescriptor, Redactor,
    RelativeTimeDescriptor,
};
use lib::git::{CategorizedReferenceName, GitRunInfo, MaybeZeroOid, Repo, ResolvedReferenceInfo};

fn render_cursor_smartlog(
    effects: &Effects,
    repo: &Repo,
    dag: &Dag,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
) -> eyre::Result<Vec<StyledString>> {
    let mut dag = dag.set_cursor(effects, repo, event_replayer, event_cursor)?;
    let references_snapshot = event_replayer.get_references_snapshot(repo, event_cursor)?;
    dag.sync_from_oids(
        effects,
        repo,
        CommitSet::from(references_snapshot.main_branch_oid),
        references_snapshot
            .branch_oid_to_names
            .keys()
            .chain(references_snapshot.head_oid.iter())
            .copied()
            .collect(),
    )?;

    // FIXME: the information of which branch is checked out is not stored in
    // the event log. For now, just pretend that `HEAD` was always detached in
    // the past.
    let head_info = ResolvedReferenceInfo {
        oid: references_snapshot.head_oid,
        reference_name: None,
    };

    let commits = resolve_default_smartlog_commits(effects, repo, &mut dag)?;
    let graph = make_smartlog_graph(
        effects,
        repo,
        &dag,
        event_replayer,
        event_cursor,
        &commits,
        false,
    )?;
    let result = render_graph(
        effects,
        repo,
        &dag,
        &graph,
        references_snapshot.head_oid,
        &mut [
            &mut CommitOidDescriptor::new(true)?,
            &mut RelativeTimeDescriptor::new(repo, SystemTime::now())?,
            &mut ObsolescenceExplanationDescriptor::new(event_replayer, event_cursor)?,
            &mut BranchesDescriptor::new(
                repo,
                &head_info,
                &references_snapshot,
                &Redactor::Disabled,
            )?,
            &mut DifferentialRevisionDescriptor::new(repo, &Redactor::Disabled)?,
            &mut CommitMessageDescriptor::new(&Redactor::Disabled)?,
        ],
    )?;
    Ok(result)
}

fn describe_event(glyphs: &Glyphs, repo: &Repo, event: &Event) -> eyre::Result<Vec<StyledString>> {
    const EMPTY_EVENT_MESSAGE: &str =
        "This may be an unsupported use-case; see https://github.com/arxanas/git-branchless/issues/57";

    let result = match event {
        Event::CommitEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid,
        } => {
            vec![
                StyledStringBuilder::new()
                    .append_plain("Commit ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *commit_oid)?)
                    .build(),
                StyledString::new(),
            ]
        }

        Event::ObsoleteEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid,
        }
        | Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid: MaybeZeroOid::NonZero(commit_oid),
            new_commit_oid: MaybeZeroOid::Zero,
        } => {
            vec![
                StyledStringBuilder::new()
                    .append_plain("Hide commit ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *commit_oid)?)
                    .build(),
                StyledString::new(),
            ]
        }

        Event::UnobsoleteEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid,
        }
        | Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid: MaybeZeroOid::Zero,
            new_commit_oid: MaybeZeroOid::NonZero(commit_oid),
        } => {
            vec![
                StyledStringBuilder::new()
                    .append_plain("Unhide commit ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *commit_oid)?)
                    .build(),
                StyledString::new(),
            ]
        }

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_oid: MaybeZeroOid::Zero,
            new_oid: MaybeZeroOid::NonZero(new_oid),
            message: _,
        } if ref_name.as_str() == "HEAD" => {
            // Not sure if this can happen. When a repo is created, maybe?
            vec![
                StyledStringBuilder::new()
                    .append_plain("Check out to ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *new_oid)?)
                    .build(),
                StyledString::new(),
            ]
        }

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_oid: MaybeZeroOid::NonZero(old_oid),
            new_oid: MaybeZeroOid::NonZero(new_oid),
            message: _,
        } if ref_name.as_str() == "HEAD" => {
            vec![
                StyledStringBuilder::new()
                    .append_plain("Check out from ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *old_oid)?)
                    .build(),
                StyledStringBuilder::new()
                    .append_plain("            to ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *new_oid)?)
                    .build(),
            ]
        }

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_oid: MaybeZeroOid::Zero,
            new_oid: MaybeZeroOid::Zero,
            message: _,
        } => {
            vec![
                StyledStringBuilder::new()
                    .append_plain("Empty event for ")
                    .append_plain(CategorizedReferenceName::new(ref_name).render_full())
                    .build(),
                StyledStringBuilder::new()
                    .append_plain(EMPTY_EVENT_MESSAGE)
                    .build(),
            ]
        }

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_oid: MaybeZeroOid::Zero,
            new_oid: MaybeZeroOid::NonZero(new_oid),
            message: _,
        } => {
            vec![
                StyledStringBuilder::new()
                    .append_plain("Create ")
                    .append_plain(CategorizedReferenceName::new(ref_name).friendly_describe())
                    .append_plain(" at ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *new_oid)?)
                    .build(),
                StyledString::new(),
            ]
        }

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_oid: MaybeZeroOid::NonZero(old_oid),
            new_oid: MaybeZeroOid::Zero,
            message: _,
        } => {
            vec![
                StyledStringBuilder::new()
                    .append_plain("Delete ")
                    .append_plain(CategorizedReferenceName::new(ref_name).friendly_describe())
                    .append_plain(" at ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *old_oid)?)
                    .build(),
                StyledString::new(),
            ]
        }

        Event::RefUpdateEvent {
            timestamp: _,
            event_tx_id: _,
            ref_name,
            old_oid: MaybeZeroOid::NonZero(old_oid),
            new_oid: MaybeZeroOid::NonZero(new_oid),
            message: _,
        } => {
            let ref_name = CategorizedReferenceName::new(ref_name).friendly_describe();
            vec![
                StyledStringBuilder::new()
                    .append_plain("Move ")
                    .append_plain(ref_name.clone())
                    .append_plain(" from ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *old_oid)?)
                    .build(),
                StyledStringBuilder::new()
                    .append_plain("     ")
                    .append_plain(" ".repeat(ref_name.len()))
                    .append_plain("   to ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *new_oid)?)
                    .build(),
            ]
        }

        Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid: MaybeZeroOid::NonZero(old_commit_oid),
            new_commit_oid: MaybeZeroOid::NonZero(new_commit_oid),
        } => {
            vec![
                StyledStringBuilder::new()
                    .append_plain("Rewrite commit ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *old_commit_oid)?)
                    .build(),
                StyledStringBuilder::new()
                    .append_plain("           as ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *new_commit_oid)?)
                    .build(),
            ]
        }

        Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid: MaybeZeroOid::Zero,
            new_commit_oid: MaybeZeroOid::Zero,
        } => {
            vec![
                StyledStringBuilder::new()
                    .append_plain("Empty rewrite event.")
                    .build(),
                StyledStringBuilder::new()
                    .append_plain(EMPTY_EVENT_MESSAGE)
                    .build(),
            ]
        }

        Event::WorkingCopySnapshot {
            timestamp: _,
            event_tx_id: _,
            head_oid: MaybeZeroOid::NonZero(head_oid),
            commit_oid,
            ref_name: Some(ref_name),
        } => {
            let branch_name = CategorizedReferenceName::new(ref_name);
            vec![
                StyledStringBuilder::new()
                    .append_plain("Restore snapshot for ")
                    .append_plain(branch_name.friendly_describe())
                    .build(),
                StyledStringBuilder::new()
                    .append_plain("         pointing to ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *head_oid)?)
                    .build(),
                StyledStringBuilder::new()
                    .append_plain("     backed up using ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *commit_oid)?)
                    .build(),
            ]
        }

        Event::WorkingCopySnapshot {
            timestamp: _,
            event_tx_id: _,
            head_oid: MaybeZeroOid::NonZero(head_oid),
            commit_oid,
            ref_name: None,
        } => {
            vec![
                StyledStringBuilder::new()
                    .append_plain("Restore snapshot for ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *head_oid)?)
                    .build(),
                StyledStringBuilder::new()
                    .append_plain("     backed up using ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *commit_oid)?)
                    .build(),
            ]
        }

        Event::WorkingCopySnapshot {
            timestamp: _,
            event_tx_id: _,
            head_oid: MaybeZeroOid::Zero,
            commit_oid,
            ref_name: Some(ref_name),
        } => {
            let branch_name = CategorizedReferenceName::new(ref_name);
            vec![
                StyledStringBuilder::new()
                    .append_plain("Restore snapshot for ")
                    .append_plain(branch_name.friendly_describe())
                    .build(),
                StyledStringBuilder::new()
                    .append_plain("     backed up using ")
                    .append(repo.friendly_describe_commit_from_oid(glyphs, *commit_oid)?)
                    .build(),
            ]
        }

        Event::WorkingCopySnapshot {
            timestamp: _,
            event_tx_id: _,
            head_oid: MaybeZeroOid::Zero,
            commit_oid,
            ref_name: None,
        } => {
            vec![StyledStringBuilder::new()
                .append_plain("Restore snapshot backed up using ")
                .append(repo.friendly_describe_commit_from_oid(glyphs, *commit_oid)?)
                .build()]
        }
    };
    Ok(result)
}

fn describe_events_numbered(
    glyphs: &Glyphs,
    repo: &Repo,
    events: &[Event],
) -> Result<Vec<StyledString>, eyre::Error> {
    let mut lines = Vec::new();
    for (i, event) in (1..).zip(events) {
        let num_header = format!("{i}. ");
        for (j, event_line) in (0..).zip(describe_event(glyphs, repo, event)?) {
            let prefix = if j == 0 {
                num_header.clone()
            } else {
                " ".repeat(num_header.len())
            };
            lines.push(
                StyledStringBuilder::new()
                    .append_plain(prefix)
                    .append(event_line)
                    .build(),
            );
        }
    }
    Ok(lines)
}

#[instrument(skip(siv))]
fn select_past_event(
    mut siv: CursiveRunner<Cursive>,
    effects: &Effects,
    repo: &Repo,
    dag: &Dag,
    event_replayer: &mut EventReplayer,
) -> eyre::Result<Option<EventCursor>> {
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
            cursive_core::event::Key::Enter.into(),
            Message::SelectEventIdAndQuit,
        ),
    ]
    .iter()
    .cloned()
    .for_each(|(event, message): (cursive_core::event::Event, Message)| {
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

        declare_views! {
            SmartlogView => ScrollView<TextView>,
            InfoView => TextView,
        }

        let redraw = |siv: &mut Cursive,
                      event_replayer: &mut EventReplayer,
                      event_cursor: EventCursor|
         -> eyre::Result<()> {
            let smartlog =
                render_cursor_smartlog(effects, repo, dag, event_replayer, event_cursor)?;
            SmartlogView::find(siv)
                .get_inner_mut()
                .set_content(StyledStringBuilder::from_lines(smartlog));

            let event = event_replayer.get_tx_events_before_cursor(event_cursor);
            let info_view_contents = match event {
                None => vec![StyledString::plain(
                    "There are no previous available events.",
                )],
                Some((event_id, events)) => {
                    let event_description_lines =
                        describe_events_numbered(effects.get_glyphs(), repo, events)?;
                    let relative_time_provider = RelativeTimeDescriptor::new(repo, now)?;
                    let relative_time = if relative_time_provider.is_enabled() {
                        format!(
                            " ({} ago)",
                            RelativeTimeDescriptor::describe_time_delta(
                                now,
                                events[0].get_timestamp()
                            )?
                        )
                    } else {
                        String::new()
                    };

                    let mut lines = vec![StyledStringBuilder::new()
                        .append_plain("Repo after transaction ")
                        .append_plain(events[0].get_event_tx_id().to_string())
                        .append_plain(" (event ")
                        .append_plain(event_id.to_string())
                        .append_plain(")")
                        .append_plain(relative_time)
                        .append_plain(". Press 'h' for help, 'q' to quit.")
                        .build()];
                    lines.extend(event_description_lines);
                    lines
                }
            };
            InfoView::find(siv).set_content(StyledStringBuilder::from_lines(info_view_contents));
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
                let smartlog_view: SmartlogView = ScrollView::new(TextView::new("")).into();
                let info_view: InfoView = TextView::new("").into();
                siv.add_fullscreen_layer(
                    LinearLayout::vertical()
                        .child(
                            Panel::new(smartlog_view)
                                .title("Commit graph")
                                .full_height(),
                        )
                        .child(Panel::new(ScrollView::new(info_view)).title("Events"))
                        .full_width(),
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
                                            "Invalid event ID: {text}"
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
                return Ok(Some(cursor));
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
) -> eyre::Result<Event> {
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();
    let inverse_event = match event {
        Event::CommitEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid,
        }
        | Event::UnobsoleteEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid,
        } => Event::ObsoleteEvent {
            timestamp,
            event_tx_id,
            commit_oid,
        },

        Event::ObsoleteEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid,
        } => Event::UnobsoleteEvent {
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
            old_oid: old_ref,
            new_oid: new_ref,
            message: _,
        } => Event::RefUpdateEvent {
            timestamp,
            event_tx_id,
            ref_name,
            old_oid: new_ref,
            new_oid: old_ref,
            message: None,
        },

        // This isn't really an "invertible" event, in that there's no way to
        // calculate an inverse event that restores the working copy state to
        // *before* this snapshot.
        //
        // Instead, we're indicating that to undo to the point in the past when
        // this event occurred, we want to check out the working copy as it
        // existed at that point in time.
        event @ Event::WorkingCopySnapshot { .. } => event,
    };
    Ok(inverse_event)
}

#[derive(Clone, Debug)]
struct UndoCheckoutTarget {
    target: CheckoutTarget,
    options: CheckOutCommitOptions,
}

fn extract_checkout_target(
    events: &[Event],
) -> eyre::Result<(Option<UndoCheckoutTarget>, Vec<&Event>)> {
    let mut new_events = Vec::new();
    let mut checkout_target = None;
    for event in events.iter() {
        match event {
            Event::RefUpdateEvent {
                timestamp: _,
                event_tx_id: _,
                ref_name,
                old_oid: _,
                new_oid: MaybeZeroOid::NonZero(new_oid),
                message: _,
            } if ref_name.as_str() == "HEAD" => {
                checkout_target = Some(UndoCheckoutTarget {
                    target: CheckoutTarget::Oid(*new_oid),
                    options: CheckOutCommitOptions {
                        additional_args: vec!["--detach".into()],
                        reset: false,
                        render_smartlog: true,
                    },
                });
            }

            Event::WorkingCopySnapshot {
                timestamp: _,
                event_tx_id: _,
                head_oid: _,
                commit_oid,
                ref_name,
            } => {
                checkout_target = Some(UndoCheckoutTarget {
                    target: CheckoutTarget::Oid(*commit_oid),
                    options: CheckOutCommitOptions {
                        additional_args: match ref_name {
                            Some(ref_name) => {
                                let branch_name = CategorizedReferenceName::new(ref_name);
                                vec!["-B".into(), branch_name.render_suffix().into()]
                            }
                            None => Default::default(),
                        },
                        reset: false,
                        render_smartlog: true,
                    },
                })
            }

            event => new_events.push(event),
        };
    }

    Ok((checkout_target, new_events))
}

#[instrument(skip(in_))]
fn undo_events(
    in_: &mut impl Read,
    effects: &Effects,
    repo: &Repo,
    git_run_info: &GitRunInfo,
    event_log_db: &mut EventLogDb,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    skip_confirmation: bool,
) -> EyreExitOr<()> {
    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "undo")?;
    let head_info = repo.get_head_info()?;
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
                    old_oid: MaybeZeroOid::Zero,
                    new_oid: _,
                    message: _,
                } if ref_name.as_str() == "HEAD"
            )
        })
        .map(|event| inverse_event(event.clone(), now, event_tx_id))
        .collect::<eyre::Result<Vec<Event>>>()?;

    if inverse_events.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "No undo actions to apply, exiting."
        )?;
        return Ok(Ok(()));
    }
    writeln!(effects.get_output_stream(), "Will apply these actions:")?;
    let events = describe_events_numbered(effects.get_glyphs(), repo, &inverse_events)?;
    for line in events {
        writeln!(
            effects.get_output_stream(),
            "{}",
            effects.get_glyphs().render(line)?
        )?;
    }

    let confirmed = if skip_confirmation {
        true
    } else {
        write!(effects.get_output_stream(), "Confirm? [yN] ")?;
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
        writeln!(effects.get_output_stream(), "Aborted.")?;
        return Ok(Err(ExitCode(1)));
    }

    let num_inverse_events = Pluralize {
        determiner: None,
        amount: inverse_events.len(),
        unit: ("inverse event", "inverse events"),
    }
    .to_string();

    let (checkout_target, filtered_events) = extract_checkout_target(&inverse_events)?;
    if checkout_target.is_some() {
        repo.detach_head(&head_info)?;
    }
    for event in filtered_events.into_iter() {
        match event {
            Event::RefUpdateEvent {
                timestamp: _,
                event_tx_id: _,
                ref_name: _,
                old_oid: MaybeZeroOid::Zero,
                new_oid: MaybeZeroOid::Zero,
                message: _,
            } => {
                // Do nothing.
            }
            Event::RefUpdateEvent {
                timestamp: _,
                event_tx_id: _,
                ref_name,
                old_oid: MaybeZeroOid::NonZero(_),
                new_oid: MaybeZeroOid::Zero,
                message: _,
            } => match repo.find_reference(ref_name)? {
                Some(mut reference) => {
                    reference.delete().wrap_err("Applying `RefUpdateEvent`")?;
                }
                None => {
                    writeln!(
                        effects.get_output_stream(),
                        "Reference {} did not exist, not deleting it.",
                        ref_name.as_str()
                    )?;
                }
            },
            Event::RefUpdateEvent {
                timestamp: _,
                event_tx_id: _,
                ref_name,
                old_oid: MaybeZeroOid::Zero,
                new_oid: MaybeZeroOid::NonZero(new_oid),
                message: _,
            }
            | Event::RefUpdateEvent {
                timestamp: _,
                event_tx_id: _,
                ref_name,
                old_oid: MaybeZeroOid::NonZero(_),
                new_oid: MaybeZeroOid::NonZero(new_oid),
                message: _,
            } => {
                // Create or update the given reference.
                repo.create_reference(ref_name, *new_oid, true, "branchless undo")?;
            }

            Event::WorkingCopySnapshot { .. } => {
                // Should be handled as the checkout target already.
            }

            Event::CommitEvent { .. }
            | Event::ObsoleteEvent { .. }
            | Event::UnobsoleteEvent { .. }
            | Event::RewriteEvent { .. } => {
                event_log_db.add_events(vec![event.clone()])?;
            }
        }
    }

    if let Some(UndoCheckoutTarget { target, options }) = checkout_target {
        try_exit_code!(check_out_commit(
            effects,
            git_run_info,
            repo,
            event_log_db,
            event_tx_id,
            Some(target),
            &options,
        )
        .wrap_err("Updating to previous HEAD location")?);
    }

    writeln!(effects.get_output_stream(), "Applied {num_inverse_events}.")?;
    Ok(Ok(()))
}

/// Restore the repository to a previous state interactively.
#[instrument]
pub fn undo(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    interactive: bool,
    skip_confirmation: bool,
) -> EyreExitOr<()> {
    let repo = Repo::from_current_dir()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let mut event_log_db = EventLogDb::new(&conn)?;
    let mut event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let dag = {
        // Don't let `event_cursor` leak from this scope, since we intend to
        // determine a new event cursor below.
        let event_cursor = event_replayer.make_default_cursor();

        Dag::open_and_sync(
            effects,
            &repo,
            &event_replayer,
            event_cursor,
            &references_snapshot,
        )?
    };

    let event_cursor = {
        if interactive {
            let result = with_siv(effects, |effects, siv| {
                select_past_event(siv, &effects, &repo, &dag, &mut event_replayer)
            })?;
            match result {
                Some(event_cursor) => event_cursor,
                None => return Ok(Ok(())),
            }
        } else {
            event_replayer.advance_cursor_by_transaction(event_replayer.make_default_cursor(), -1)
        }
    };

    let result = undo_events(
        &mut stdin(),
        effects,
        &repo,
        git_run_info,
        &mut event_log_db,
        &event_replayer,
        event_cursor,
        skip_confirmation,
    )?;
    Ok(result)
}

#[allow(missing_docs)]
pub mod testing {
    use std::io::Read;

    use cursive_core::{Cursive, CursiveRunner};

    use lib::core::dag::Dag;
    use lib::core::effects::Effects;
    use lib::core::eventlog::{EventCursor, EventLogDb, EventReplayer};
    use lib::git::{GitRunInfo, Repo};
    use lib::util::EyreExitOr;

    pub fn select_past_event(
        siv: CursiveRunner<Cursive>,
        effects: &Effects,
        repo: &Repo,
        dag: &Dag,
        event_replayer: &mut EventReplayer,
    ) -> eyre::Result<Option<EventCursor>> {
        super::select_past_event(siv, effects, repo, dag, event_replayer)
    }

    pub fn undo_events(
        in_: &mut impl Read,
        effects: &Effects,
        repo: &Repo,
        git_run_info: &GitRunInfo,
        event_log_db: &mut EventLogDb,
        event_replayer: &EventReplayer,
        event_cursor: EventCursor,
    ) -> EyreExitOr<()> {
        super::undo_events(
            in_,
            effects,
            repo,
            git_run_info,
            event_log_db,
            event_replayer,
            event_cursor,
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use lib::core::eventlog::testing::new_event_transaction_id;

    #[test]
    fn test_optimize_inverse_events() -> eyre::Result<()> {
        let event_tx_id = new_event_transaction_id(123);
        let input = vec![
            Event::RefUpdateEvent {
                timestamp: 1.0,
                event_tx_id,
                ref_name: "HEAD".into(),
                old_oid: MaybeZeroOid::NonZero("1".parse()?),
                new_oid: MaybeZeroOid::NonZero("2".parse()?),
                message: None,
            },
            Event::RefUpdateEvent {
                timestamp: 2.0,
                event_tx_id,
                ref_name: "HEAD".into(),
                old_oid: MaybeZeroOid::NonZero("1".parse()?),
                new_oid: MaybeZeroOid::NonZero("3".parse()?),
                message: None,
            },
        ];
        insta::assert_debug_snapshot!(extract_checkout_target(&input)?, @r###"
        (
            Some(
                UndoCheckoutTarget {
                    target: Oid(
                        NonZeroOid(3000000000000000000000000000000000000000),
                    ),
                    options: CheckOutCommitOptions {
                        additional_args: [
                            "--detach",
                        ],
                        reset: false,
                        render_smartlog: true,
                    },
                },
            ),
            [],
        )
        "###);
        Ok(())
    }
}
