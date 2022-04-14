use std::convert::Infallible;
use std::mem::swap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::util::trim_lines;

use git_branchless::commands::testing::undo::{select_past_event, undo_events};
use git_branchless::tui::testing::{screen_to_string, CursiveTestingBackend, CursiveTestingEvent};
use lib::core::dag::Dag;
use lib::core::effects::Effects;
use lib::core::eventlog::{EventCursor, EventLogDb, EventReplayer};
use lib::core::formatting::Glyphs;
use lib::git::{GitRunInfo, GitVersion, Repo};
use lib::testing::{make_git, Git};

use cursive::event::Key;
use cursive::CursiveRunnable;

fn run_select_past_event(
    repo: &Repo,
    events: Vec<CursiveTestingEvent>,
) -> eyre::Result<Option<EventCursor>> {
    let glyphs = Glyphs::text();
    let effects = Effects::new_suppress_for_test(glyphs);
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db: EventLogDb = EventLogDb::new(&conn)?;
    let mut event_replayer = EventReplayer::from_event_log_db(&effects, repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let dag = Dag::open_and_sync(
        &effects,
        repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;
    let siv = CursiveRunnable::new::<Infallible, _>(move || {
        Ok(CursiveTestingBackend::init(events.clone()))
    });
    select_past_event(siv.into_runner(), &effects, repo, &dag, &mut event_replayer)
}

fn run_undo_events(git: &Git, event_cursor: EventCursor) -> eyre::Result<(isize, String)> {
    let glyphs = Glyphs::text();
    let effects = Effects::new_suppress_for_test(glyphs.clone());
    let repo = git.get_repo()?;
    let conn = repo.get_db_conn()?;
    let mut event_log_db: EventLogDb = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(&effects, &repo, &event_log_db)?;
    let input = "y";
    let mut in_ = input.as_bytes();
    let stdout: Arc<Mutex<Vec<u8>>> = Default::default();
    let stderr: Arc<Mutex<Vec<u8>>> = Default::default();

    let git_run_info = GitRunInfo {
        path_to_git: git.path_to_git.clone(),
        working_directory: repo.get_working_copy_path().unwrap().to_path_buf(),
        env: git.get_base_env(0).into_iter().collect(),
    };

    let exit_code = undo_events(
        &mut in_,
        &Effects::new_from_buffer_for_test(glyphs, &stdout, &stderr),
        &repo,
        &git_run_info,
        &mut event_log_db,
        &event_replayer,
        event_cursor,
    )?;

    let stdout = {
        let mut buf = stdout.lock().unwrap();
        let mut result_buf = Vec::new();
        swap(&mut *buf, &mut result_buf);
        result_buf
    };
    let stdout = String::from_utf8(stdout)?;
    let stdout = git.preprocess_output(stdout)?;
    let stdout = trim_lines(stdout);
    Ok((exit_code, stdout))
}

#[test]
fn test_undo_help() -> eyre::Result<()> {
    let git = make_git()?;

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
        ┌───────────────────────────────────────────────────┤ Commit graph ├───────────────────────────────────────────────────┐
        │O f777ecc (master) create initial.txt                                                                                 │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │        ┌───────────────────────────────────────────┤ How to use ├───────────────────────────────────────────┐        │
        │        │ Use `git undo` to view and revert to previous states of the repository.                            │        │
        │        │                                                                                                    │        │
        │        │ h/?: Show this help.                                                                               │        │
        │        │ q: Quit.                                                                                           │        │
        │        │ p/n or <left>/<right>: View next/previous state.                                                   │        │
        │        │ g: Go to a provided event ID.                                                                      │        │
        │        │ <enter>: Revert the repository to the given state (requires confirmation).                         │        │
        │        │                                                                                                    │        │
        │        │ You can also copy a commit hash from the past and manually run `git unhide` or `git rebase` on it. │        │
        │        │                                                                                                    │        │
        │        │                                                                                            <Close> │        │
        │        └────────────────────────────────────────────────────────────────────────────────────────────────────┘        │
        │                                                                                                                      │
        │                                                                                                                      │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        ┌──────────────────────────────────────────────────────┤ Events ├──────────────────────────────────────────────────────┐
        │There are no previous available events.                                                                               │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        "###);
    }

    Ok(())
}

#[test]
fn test_undo_navigate() -> eyre::Result<()> {
    let git = make_git()?;

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
        ┌───────────────────────────────────────────────────┤ Commit graph ├───────────────────────────────────────────────────┐
        │:                                                                                                                     │
        │@ 96d1c37 (master) create test2.txt                                                                                   │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        ┌──────────────────────────────────────────────────────┤ Events ├──────────────────────────────────────────────────────┐
        │Repo after transaction 3 (event 4). Press 'h' for help, 'q' to quit.                                                  │
        │1. Check out from 62fc20d create test1.txt                                                                            │
        │               to 96d1c37 create test2.txt                                                                            │
        │2. Move branch master from 62fc20d create test1.txt                                                                   │
        │                        to 96d1c37 create test2.txt                                                                   │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot2), @r###"
        ┌───────────────────────────────────────────────────┤ Commit graph ├───────────────────────────────────────────────────┐
        │:                                                                                                                     │
        │@ 96d1c37 (master) create test2.txt                                                                                   │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        ┌──────────────────────────────────────────────────────┤ Events ├──────────────────────────────────────────────────────┐
        │Repo after transaction 4 (event 6). Press 'h' for help, 'q' to quit.                                                  │
        │1. Commit 96d1c37 create test2.txt                                                                                    │
        │                                                                                                                      │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        "###);
    };

    Ok(())
}

#[test]
fn test_go_to_event() -> eyre::Result<()> {
    let git = make_git()?;

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
    ┌───────────────────────────────────────────────────┤ Commit graph ├───────────────────────────────────────────────────┐
    │:                                                                                                                     │
    │@ 96d1c37 (master) create test2.txt                                                                                   │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
    ┌──────────────────────────────────────────────────────┤ Events ├──────────────────────────────────────────────────────┐
    │Repo after transaction 4 (event 6). Press 'h' for help, 'q' to quit.                                                  │
    │1. Commit 96d1c37 create test2.txt                                                                                    │
    │                                                                                                                      │
    └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
    "###);
    insta::assert_snapshot!(screen_to_string(&screenshot2), @r###"
    ┌───────────────────────────────────────────────────┤ Commit graph ├───────────────────────────────────────────────────┐
    │:                                                                                                                     │
    │@ 62fc20d create test1.txt                                                                                            │
    │|                                                                                                                     │
    │O 96d1c37 (master) create test2.txt                                                                                   │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
    ┌──────────────────────────────────────────────────────┤ Events ├──────────────────────────────────────────────────────┐
    │Repo after transaction 1 (event 1). Press 'h' for help, 'q' to quit.                                                  │
    │1. Check out from f777ecc create initial.txt                                                                          │
    │               to 62fc20d create test1.txt                                                                            │
    └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
    "###);

    Ok(())
}

#[test]
fn test_undo_hide() -> eyre::Result<()> {
    let git = make_git()?;

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
        O f777ecc (master) create initial.txt
        |
        @ fe65c1f create test2.txt
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
        let (exit_code, stdout) = run_undo_events(&git, event_cursor)?;
        insta::assert_snapshot!(stdout, @r###"
        Will apply these actions:
        1. Create branch test1 at 62fc20d create test1.txt

        2. Unhide commit 62fc20d create test1.txt

        Confirm? [yN] Applied 2 inverse events.
        "###);
        assert_eq!(exit_code, 0);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc (master) create initial.txt
        |\
        | o 62fc20d (test1) create test1.txt
        |
        @ fe65c1f create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_undo_move_refs() -> eyre::Result<()> {
    let git = make_git()?;

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
        let (exit_code, stdout) = run_undo_events(&git, event_cursor)?;
        insta::assert_snapshot!(stdout, @r###"
        Will apply these actions:
        1. Check out from 96d1c37 create test2.txt
                       to 62fc20d create test1.txt
        2. Hide commit 96d1c37 create test2.txt

        3. Move branch master from 96d1c37 create test2.txt
                                to 62fc20d create test1.txt
        Confirm? [yN] branchless: running command: <git-executable> checkout 62fc20d2a290daea0d52bdc2ed2ad4be6491010e --detach
        Applied 3 inverse events.
        "###);
        assert_eq!(exit_code, 0);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d (master) create test1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_historical_smartlog_visibility() -> eyre::Result<()> {
    let git = make_git()?;

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

    if git.supports_reference_transactions()? {
        insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
        ┌───────────────────────────────────────────────────┤ Commit graph ├───────────────────────────────────────────────────┐
        │:                                                                                                                     │
        │% 62fc20d (manually hidden) (master) create test1.txt                                                                 │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        ┌──────────────────────────────────────────────────────┤ Events ├──────────────────────────────────────────────────────┐
        │Repo after transaction 3 (event 4). Press 'h' for help, 'q' to quit.                                                  │
        │1. Hide commit 62fc20d create test1.txt                                                                               │
        │                                                                                                                      │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot2), @r###"
        ┌───────────────────────────────────────────────────┤ Commit graph ├───────────────────────────────────────────────────┐
        │:                                                                                                                     │
        │@ 62fc20d (master) create test1.txt                                                                                   │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        ┌──────────────────────────────────────────────────────┤ Events ├──────────────────────────────────────────────────────┐
        │Repo after transaction 2 (event 3). Press 'h' for help, 'q' to quit.                                                  │
        │1. Commit 62fc20d create test1.txt                                                                                    │
        │                                                                                                                      │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        "###);
    } else {
        insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
        ┌───────────────────────────────────────────────────┤ Commit graph ├───────────────────────────────────────────────────┐
        │:                                                                                                                     │
        │% 62fc20d (manually hidden) (master) create test1.txt                                                                 │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        ┌──────────────────────────────────────────────────────┤ Events ├──────────────────────────────────────────────────────┐
        │Repo after transaction 2 (event 2). Press 'h' for help, 'q' to quit.                                                  │
        │1. Hide commit 62fc20d create test1.txt                                                                               │
        │                                                                                                                      │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        "###);
        insta::assert_snapshot!(screen_to_string(&screenshot2), @r###"
        ┌───────────────────────────────────────────────────┤ Commit graph ├───────────────────────────────────────────────────┐
        │:                                                                                                                     │
        │@ 62fc20d (master) create test1.txt                                                                                   │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        │                                                                                                                      │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        ┌──────────────────────────────────────────────────────┤ Events ├──────────────────────────────────────────────────────┐
        │Repo after transaction 1 (event 1). Press 'h' for help, 'q' to quit.                                                  │
        │1. Commit 62fc20d create test1.txt                                                                                    │
        │                                                                                                                      │
        └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
        "###);
    }

    Ok(())
}

#[test]
fn test_undo_doesnt_make_working_dir_dirty() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;

    // Modify a reference.
    git.run(&["branch", "foo"])?;
    // Make a change that causes a checkout.
    git.commit_file("test1", 1)?;
    // Modify a reference.
    git.run(&["branch", "bar"])?;

    let screenshot1 = Default::default();
    let event_cursor = run_select_past_event(
        &git.get_repo()?,
        vec![
            CursiveTestingEvent::Event('p'.into()),
            CursiveTestingEvent::Event('p'.into()),
            CursiveTestingEvent::Event('p'.into()),
            CursiveTestingEvent::Event('p'.into()),
            CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
            CursiveTestingEvent::Event(Key::Enter.into()),
        ],
    )?;
    insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
    ┌───────────────────────────────────────────────────┤ Commit graph ├───────────────────────────────────────────────────┐
    │:                                                                                                                     │
    │O 62fc20d (master) create test1.txt                                                                                   │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
    ┌──────────────────────────────────────────────────────┤ Events ├──────────────────────────────────────────────────────┐
    │There are no previous available events.                                                                               │
    └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
    "###);

    // If there are no dirty files in the repository prior to the `undo`,
    // then there should still be no dirty files after the `undo`.
    let event_cursor = event_cursor.expect("Should have an event cursor to undo");
    {
        let (stdout, _stderr) = git.run(&["status", "--porcelain"])?;
        assert_eq!(stdout, "");
    }
    {
        let (exit_code, stdout) = run_undo_events(&git, event_cursor)?;
        insta::assert_snapshot!(stdout, @r###"
        Will apply these actions:
        1. Check out from 62fc20d create test1.txt
                       to f777ecc create initial.txt
        2. Delete branch bar at 62fc20d create test1.txt

        3. Hide commit 62fc20d create test1.txt

        4. Move branch master from 62fc20d create test1.txt
                                to f777ecc create initial.txt
        5. Delete branch foo at f777ecc create initial.txt

        Confirm? [yN] branchless: running command: <git-executable> checkout f777ecc9b0db5ed372b2615695191a8a17f79f24 --detach
        Applied 5 inverse events.
        "###);
        assert_eq!(exit_code, 0);
    }
    {
        let (stdout, _stderr) = git.run(&["status", "--porcelain"])?;
        assert_eq!(stdout, "");
    }

    Ok(())
}

/// See https://github.com/arxanas/git-branchless/issues/57
#[cfg(unix)]
#[test]
fn test_git_bisect_produces_empty_event() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.run(&["bisect", "start"])?;
    git.run(&["bisect", "good", "HEAD^"])?;
    git.run(&["bisect", "bad"])?;

    let screenshot1 = Default::default();
    run_select_past_event(
        &git.get_repo()?,
        vec![
            CursiveTestingEvent::Event('p'.into()),
            CursiveTestingEvent::Event('p'.into()),
            CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
            CursiveTestingEvent::Event(Key::Enter.into()),
        ],
    )?;
    insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
    ┌───────────────────────────────────────────────────┤ Commit graph ├───────────────────────────────────────────────────┐
    │:                                                                                                                     │
    │@ 62fc20d (master) create test1.txt                                                                                   │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
    ┌──────────────────────────────────────────────────────┤ Events ├──────────────────────────────────────────────────────┐
    │Repo after transaction 3 (event 4). Press 'h' for help, 'q' to quit.                                                  │
    │1. Empty event for BISECT_HEAD                                                                                        │
    │   This may be an unsupported use-case; see https://git.io/J0b7z                                                      │
    └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
    "###);

    Ok(())
}

#[test]
fn test_undo_garbage_collected_commit() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    let git_version = git.get_version()?;
    if git_version >= GitVersion(2, 35, 0) {
        // Change in reference-transaction behavior causes this test to fail.
        return Ok(());
    }

    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.run(&["hide", "HEAD"])?;
    git.run(&["checkout", "HEAD^"])?;

    {
        let (stdout, _stderr) = git.run(&["branchless", "gc"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: collecting garbage
        branchless: 1 dangling reference deleted
        "###);
    }
    git.run(&["gc", "--prune=now"])?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d (master) create test1.txt
        "###);
    }

    let screenshot1 = Default::default();
    let event_cursor = run_select_past_event(
        &git.get_repo()?,
        vec![
            CursiveTestingEvent::Event('p'.into()),
            CursiveTestingEvent::Event('p'.into()),
            CursiveTestingEvent::Event('p'.into()),
            CursiveTestingEvent::Event('p'.into()),
            CursiveTestingEvent::TakeScreenshot(Rc::clone(&screenshot1)),
            CursiveTestingEvent::Event(Key::Enter.into()),
        ],
    )?;
    insta::assert_snapshot!(screen_to_string(&screenshot1), @r###"
    ┌───────────────────────────────────────────────────┤ Commit graph ├───────────────────────────────────────────────────┐
    │:                                                                                                                     │
    │O 62fc20d (master) create test1.txt                                                                                   │
    │|                                                                                                                     │
    │% 96d1c37 (manually hidden) <garbage collected>                                                                       │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    │                                                                                                                      │
    └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
    ┌──────────────────────────────────────────────────────┤ Events ├──────────────────────────────────────────────────────┐
    │Repo after transaction 7 (event 8). Press 'h' for help, 'q' to quit.                                                  │
    │1. Hide commit <commit not available: 96d1c37a3d4363611c49f7e52186e189a04c531f>                                       │
    │                                                                                                                      │
    └──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
    "###);

    let event_cursor = event_cursor.unwrap();
    {
        let (exit_code, stdout) = run_undo_events(&git, event_cursor)?;
        insta::assert_snapshot!(stdout, @r###"
        Will apply these actions:
        1. Check out from 62fc20d create test1.txt
                       to <commit not available: 96d1c37a3d4363611c49f7e52186e189a04c531f>
        2. Move branch master from 62fc20d create test1.txt
                                to 62fc20d create test1.txt
        3. Move branch master from 62fc20d create test1.txt
                                to 62fc20d create test1.txt
        Confirm? [yN] branchless: running command: <git-executable> checkout 96d1c37a3d4363611c49f7e52186e189a04c531f --detach
        Failed to check out commit: 96d1c37a3d4363611c49f7e52186e189a04c531f
        Applied 3 inverse events.
        "###);
        assert!(exit_code > 0);
    }

    Ok(())
}

#[test]
fn test_undo_noninteractive() -> eyre::Result<()> {
    let git = make_git()?;

    if !git.supports_reference_transactions()? {
        return Ok(());
    }

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&[
        "branchless",
        "wrap",
        "--",
        "commit",
        "--amend",
        "-m",
        "bad message",
    ])?;

    {
        let (stdout, _stderr) = git.run_with_options(
            &["undo"],
            &lib::testing::GitRunOptions {
                expected_exit_code: 1,
                input: Some("n".to_string()),
                ..Default::default()
            },
        )?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        Will apply these actions:
        1. Check out from 9ed8f9a bad message
                       to 96d1c37 create test2.txt
        2. Rewrite commit 9ed8f9a bad message
                      as 96d1c37 create test2.txt
        3. Hide commit 9ed8f9a bad message

        4. Move branch master from 9ed8f9a bad message
                                to 96d1c37 create test2.txt
        Confirm? [yN] Aborted.
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 9ed8f9a (> master) bad message
        "###);
    }

    {
        let (stdout, _stderr) = git.run_with_options(
            &["undo"],
            &lib::testing::GitRunOptions {
                input: Some("y".to_string()),
                ..Default::default()
            },
        )?;
        let stdout = trim_lines(stdout);
        insta::assert_snapshot!(stdout, @r###"
        Will apply these actions:
        1. Check out from 9ed8f9a bad message
                       to 96d1c37 create test2.txt
        2. Rewrite commit 9ed8f9a bad message
                      as 96d1c37 create test2.txt
        3. Hide commit 9ed8f9a bad message

        4. Move branch master from 9ed8f9a bad message
                                to 96d1c37 create test2.txt
        Confirm? [yN] branchless: running command: <git-executable> checkout 96d1c37a3d4363611c49f7e52186e189a04c531f --detach
        :
        O 62fc20d create test1.txt
        |\
        | % 96d1c37 (rewritten as 9ed8f9a2) create test2.txt
        |
        O 9ed8f9a (master) bad message
        Applied 4 inverse events.
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 96d1c37 (master) create test2.txt
        "###);
    }

    Ok(())
}
