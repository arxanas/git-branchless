use std::borrow::Borrow;
use std::cell::RefCell;
use std::convert::Infallible;

use std::rc::Rc;

use branchless::commands::undo::testing::{select_past_event, undo_events};
use branchless::core::eventlog::{EventCursor, EventLogDb, EventReplayer};
use branchless::core::formatting::Glyphs;
use branchless::core::mergebase::MergeBaseDb;
use branchless::testing::{with_git, Git};
use branchless::util::{get_db_conn, GitExecutable};

use cursive::backend::Backend;
use cursive::event::Key;
use cursive::theme::Color;
use cursive::CursiveRunnable;

type Screen = Vec<Vec<char>>;

#[derive(Clone, Debug)]
enum CursiveTestingEvent {
    Event(cursive::event::Event),
    TakeScreenshot(Rc<RefCell<Screen>>),
}

#[derive(Debug)]
struct CursiveTestingBackend {
    events: Vec<CursiveTestingEvent>,
    event_index: usize,
    just_emitted_event: bool,
    screen: RefCell<Screen>,
    screenshots: Vec<Screen>,
}

impl<'screenshot> CursiveTestingBackend {
    fn init(events: Vec<CursiveTestingEvent>) -> Box<dyn Backend> {
        Box::new(CursiveTestingBackend {
            events,
            event_index: 0,
            just_emitted_event: false,
            screen: RefCell::new(vec![vec![' '; 80]; 24]),
            screenshots: Vec::new(),
        })
    }
}

impl Backend for CursiveTestingBackend {
    fn poll_event(&mut self) -> Option<cursive::event::Event> {
        // Cursive will poll all available events. We only want it to
        // process events one at a time, so return `None` after each event.
        if self.just_emitted_event {
            self.just_emitted_event = false;
            return None;
        }

        let event_index = self.event_index;
        self.event_index += 1;
        match self.events.get(event_index)?.to_owned() {
            CursiveTestingEvent::TakeScreenshot(screen_target) => {
                let mut screen_target = (*screen_target).borrow_mut();
                *screen_target = self.screen.borrow().clone();
                self.poll_event()
            }
            CursiveTestingEvent::Event(event) => {
                self.just_emitted_event = true;
                Some(event.to_owned())
            }
        }
    }

    fn refresh(&mut self) {}

    fn has_colors(&self) -> bool {
        false
    }

    fn screen_size(&self) -> cursive::Vec2 {
        let screen = self.screen.borrow();
        (screen[0].len(), screen.len()).into()
    }

    fn print_at(&self, pos: cursive::Vec2, text: &str) {
        for (i, c) in text.char_indices() {
            let mut screen = self.screen.borrow_mut();
            screen[pos.y][pos.x + i] = c;
        }
    }

    fn clear(&self, _color: Color) {
        let mut screen = self.screen.borrow_mut();
        for i in 0..screen.len() {
            for j in 0..screen[i].len() {
                screen[i][j] = ' ';
            }
        }
    }

    fn set_color(&self, colors: cursive::theme::ColorPair) -> cursive::theme::ColorPair {
        colors
    }

    fn set_effect(&self, _effect: cursive::theme::Effect) {}

    fn unset_effect(&self, _effect: cursive::theme::Effect) {}
}

fn run_select_past_event(
    repo: &git2::Repository,
    events: Vec<CursiveTestingEvent>,
) -> anyhow::Result<Option<EventCursor>> {
    let glyphs = Glyphs::text();
    let conn = get_db_conn(&repo)?;
    let merge_base_db = MergeBaseDb::new(&conn)?;
    let event_log_db: EventLogDb = EventLogDb::new(&conn)?;
    let mut event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
    let siv = CursiveRunnable::new::<Infallible, _>(move || {
        Ok(CursiveTestingBackend::init(events.clone()))
    });
    select_past_event(
        siv.into_runner(),
        &glyphs,
        &repo,
        &merge_base_db,
        &mut event_replayer,
    )
}

fn run_undo_events(git: &Git, event_cursor: EventCursor) -> anyhow::Result<(String, String)> {
    let repo = git.get_repo()?;
    let conn = get_db_conn(&repo)?;
    let mut event_log_db: EventLogDb = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
    let input = "y";
    let mut in_ = input.as_bytes();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = undo_events(
        &mut in_,
        &mut out,
        &mut err,
        &repo,
        &GitExecutable(git.git_executable.clone()),
        &mut event_log_db,
        &event_replayer,
        event_cursor,
    )?;
    assert_eq!(result, 0);

    let out = String::from_utf8(out)?;
    let out = git.preprocess_stdout(out)?;
    let out = console::strip_ansi_codes(&out).to_string();
    let out = trim_lines(out);
    let err = String::from_utf8(err)?;
    let err = git.preprocess_stdout(err)?;
    let err = console::strip_ansi_codes(&err).to_string();
    let err = trim_lines(err);
    Ok((out, err))
}

fn screen_to_string(screen: &Rc<RefCell<Screen>>) -> String {
    let screen = Rc::borrow(screen);
    let screen = RefCell::borrow(screen);
    screen
        .iter()
        .map(|row| {
            let line: String = row.iter().collect();
            let line = console::strip_ansi_codes(&line);
            line.trim().to_owned() + "\n"
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

fn trim_lines(output: String) -> String {
    output
        .lines()
        .flat_map(|line| vec![line.trim_end(), "\n"].into_iter())
        .collect()
}

#[test]
fn test_undo_help() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;

        {
            let screenshot1 = Default::default();
            run_select_past_event(
                &git.get_repo()?,
                vec![
                    CursiveTestingEvent::Event('h'.into()),
                    CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
                    CursiveTestingEvent::Event('q'.into()),
                ],
            )?;
            insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
            ┌───────────────────────────────┤─How to use ├───────────────────────────────┐
            │ Use `git undo` to view and revert to previous states of the repository.    │
            │                                                                            │
            │ h/?: Show this help.                                                       │
            │ q: Quit.                                                                   │
            │ p/n or <left>/<right>: View next/previous state.                           │
            │ g: Go to a provided event ID.                                              │
            │ <enter>: Revert the repository to the given state (requires confirmation). │
            │                                                                            │
            │ You can also copy a commit hash from the past and manually run `git        │
            │ unhide` or `git rebase` on it.                                             │
            │                                                                            │
            │                                                                    <Close> │
            └────────────────────────────────────────────────────────────────────────────┘
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_undo_navigate() -> anyhow::Result<()> {
    with_git(|git| {
        if !git.supports_reference_transactions()? {
            return Ok(());
        }

        git.init_repo()?;
        git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;

        {
            let screenshot1 = Default::default();
            let screenshot2 = Default::default();
            let event_cursor = run_select_past_event(
                &git.get_repo()?,
                vec![
                    CursiveTestingEvent::Event('p'.into()),
                    CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
                    CursiveTestingEvent::Event('n'.into()),
                    CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot2)),
                    CursiveTestingEvent::Event(Key::Enter.into()),
                ],
            )?;
            insta::assert_debug_snapshot!(event_cursor, @r###"
            Some(
                EventCursor {
                    event_id: 6,
                },
            )
            "###);
            insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
            :
            @ 96d1c37a (master) create test2.txt
            Repo after transaction 3 (event 4). Press 'h' for help, 'q' to quit.
            1. Check out from 62fc20d2 create test1.txt
            to 96d1c37a create test2.txt
            2. Move branch master from 62fc20d2 create test1.txt
            to 96d1c37a create test2.txt
            "###);
            insta::assert_snapshot!(screen_to_string(&screenshot2), @r###"
            :
            @ 96d1c37a (master) create test2.txt
            Repo after transaction 4 (event 6). Press 'h' for help, 'q' to quit.
            1. Commit 96d1c37a create test2.txt
            "###);
        };

        Ok(())
    })
}

#[test]
fn test_go_to_event() -> anyhow::Result<()> {
    with_git(|git| {
        if !git.supports_reference_transactions()? {
            return Ok(());
        }

        git.init_repo()?;
        git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;

        let screenshot1 = Default::default();
        let screenshot2 = Default::default();
        run_select_past_event(
            &git.get_repo()?,
            vec![
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
                CursiveTestingEvent::Event('g'.into()),
                CursiveTestingEvent::Event('1'.into()),
                CursiveTestingEvent::Event(Key::Enter.into()),
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot2)),
                CursiveTestingEvent::Event('q'.into()),
            ],
        )?;

        insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
        :
        @ 96d1c37a (master) create test2.txt
        Repo after transaction 4 (event 6). Press 'h' for help, 'q' to quit.
        1. Commit 96d1c37a create test2.txt
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot2), @r###"
        :
        @ 62fc20d2 create test1.txt
        |
        O 96d1c37a (master) create test2.txt
        Repo after transaction 1 (event 1). Press 'h' for help, 'q' to quit.
        1. Check out from f777ecc9 create initial.txt
        to 62fc20d2 create test1.txt
        "###);

        Ok(())
    })
}

#[test]
fn test_undo_hide() -> anyhow::Result<()> {
    with_git(|git| {
        if !git.supports_reference_transactions()? {
            return Ok(());
        }

        git.init_repo()?;
        git.run(&["checkout", "-b", "test1"])?;
        git.commit_file("test1", 1)?;
        git.run(&["checkout", "HEAD^"])?;
        git.commit_file("test2", 2)?;
        git.run(&["hide", "test1"])?;
        git.run(&["branch", "-D", "test1"])?;

        {
            let (stdout, stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stderr, @"");
            insta::assert_snapshot!(stdout, @r###"
            O f777ecc9 (master) create initial.txt
            |
            @ fe65c1fe create test2.txt
            "###);
        }

        let event_cursor = run_select_past_event(
            &git.get_repo()?,
            vec![
                CursiveTestingEvent::Event('p'.into()),
                CursiveTestingEvent::Event('p'.into()),
                CursiveTestingEvent::Event(Key::Enter.into()),
                CursiveTestingEvent::Event('y'.into()),
            ],
        )?;
        insta::assert_debug_snapshot!(event_cursor, @r###"
        Some(
            EventCursor {
                event_id: 9,
            },
        )
        "###);
        let event_cursor = event_cursor.unwrap();

        {
            let (stdout, stderr) = run_undo_events(&git, event_cursor)?;
            insta::assert_snapshot!(stderr, @"");
            insta::assert_snapshot!(stdout, @r###"
            Will apply these actions:
            1. Create branch test1 at 62fc20d2 create test1.txt

            2. Unhide commit 62fc20d2 create test1.txt

            Confirm? [yN] Applied 2 inverse events.
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            O f777ecc9 (master) create initial.txt
            |\
            | o 62fc20d2 (test1) create test1.txt
            |
            @ fe65c1fe create test2.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_undo_move_refs() -> anyhow::Result<()> {
    with_git(|git| {
        if !git.supports_reference_transactions()? {
            return Ok(());
        }

        git.init_repo()?;
        git.commit_file("test1", 1)?;
        git.commit_file("test2", 2)?;

        let event_cursor = run_select_past_event(
            &git.get_repo()?,
            vec![
                CursiveTestingEvent::Event('p'.into()),
                CursiveTestingEvent::Event('p'.into()),
                CursiveTestingEvent::Event(Key::Enter.into()),
                CursiveTestingEvent::Event('y'.into()),
            ],
        )?;
        insta::assert_debug_snapshot!(event_cursor, @r###"
        Some(
            EventCursor {
                event_id: 3,
            },
        )
        "###);
        let event_cursor = event_cursor.unwrap();

        {
            let (stdout, _stderr) = run_undo_events(&git, event_cursor)?;
            insta::assert_snapshot!(stdout, @r###"
            Will apply these actions:
            1. Hide commit 96d1c37a create test2.txt

            2. Move branch master from 96d1c37a create test2.txt
                                    to 62fc20d2 create test1.txt
            3. Check out from 96d1c37a create test2.txt
                           to 62fc20d2 create test1.txt
            Confirm? [yN] branchless: <git-executable> checkout 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
            Applied 3 inverse events.
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["smartlog"])?;
            insta::assert_snapshot!(stdout, @r###"
            :
            @ 62fc20d2 (master) create test1.txt
            "###);
        }

        Ok(())
    })
}

#[test]
fn test_historical_smartlog_visibility() -> anyhow::Result<()> {
    with_git(|git| {
        git.init_repo()?;
        git.commit_file("test1", 1)?;
        git.run(&["hide", "HEAD"])?;

        let screenshot1 = Default::default();
        let screenshot2 = Default::default();
        run_select_past_event(
            &git.get_repo()?,
            vec![
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
                CursiveTestingEvent::Event('p'.into()),
                CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot2)),
                CursiveTestingEvent::Event('q'.into()),
            ],
        )?;

        insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
        :
        % 62fc20d2 (master) create test1.txt
        Repo after transaction 3 (event 4). Press 'h' for help, 'q' to quit.
        1. Hide commit 62fc20d2 create test1.txt
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot2), @r###"
        :
        @ 62fc20d2 (master) create test1.txt
        Repo after transaction 2 (event 3). Press 'h' for help, 'q' to quit.
        1. Commit 62fc20d2 create test1.txt
        "###);

        Ok(())
    })
}
