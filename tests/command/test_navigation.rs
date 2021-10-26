use std::io::{Read, Write};
use std::thread;

use eyre::eyre;

use branchless::testing::{make_git, Git, GitRunOptions};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const CARRIAGE_RETURN: &'static str = "\r";
const END_OF_TEXT: &'static str = "\x03";

#[test]
fn test_prev() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;

    {
        let (stdout, _stderr) = git.run(&["prev"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout HEAD^
        @ f777ecc9 create initial.txt
        |
        O 62fc20d2 (master) create test1.txt
        "###);
    }

    {
        let (stdout, stderr) = git.run_with_options(
            &["prev"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @"branchless: running command: <git-executable> checkout HEAD^
");
        insta::assert_snapshot!(stderr, @"error: pathspec 'HEAD^' did not match any file(s) known to git
");
    }

    Ok(())
}

#[test]
fn test_prev_multiple() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = git.run(&["prev", "2"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout HEAD~2
        @ f777ecc9 create initial.txt
        :
        O 96d1c37a (master) create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_next_multiple() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "master"])?;

    {
        let (stdout, _stderr) = git.run(&["next", "2"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 96d1c37a3d4363611c49f7e52186e189a04c531f
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        @ 96d1c37a create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_next_ambiguous() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.run(&["checkout", "master"])?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "master"])?;
    git.detach_head()?;
    git.commit_file("test3", 3)?;
    git.run(&["checkout", "master"])?;

    {
        let (stdout, _stderr) = git.run_with_options(
            &["next"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
            Found multiple possible next commits to go to after traversing 0 children:
              - 62fc20d2 create test1.txt (oldest)
              - fe65c1fe create test2.txt
              - 98b9119d create test3.txt (newest)
            (Pass --oldest (-o) or --newest (-n) to select between ambiguous next commits)
            "###);
    }

    {
        let (stdout, _stderr) = git.run(&["next", "--oldest"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
        O f777ecc9 (master) create initial.txt
        |\
        | @ 62fc20d2 create test1.txt
        |\
        | o fe65c1fe create test2.txt
        |
        o 98b9119d create test3.txt
        "###);
    }

    git.run(&["checkout", "master"])?;
    {
        let (stdout, _stderr) = git.run(&["next", "--newest"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 98b9119d16974f372e76cb64a3b77c528fc0b18b
        O f777ecc9 (master) create initial.txt
        |\
        | o 62fc20d2 create test1.txt
        |\
        | o fe65c1fe create test2.txt
        |
        @ 98b9119d create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_next_on_master() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.detach_head()?;
    git.commit_file("test3", 3)?;
    git.run(&["checkout", "HEAD^^"])?;

    {
        let (stdout, _stderr) = git.run(&["next", "2"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 70deb1e28791d8e7dd5a1f0c871a51b91282562f
        :
        O 96d1c37a (master) create test2.txt
        |
        @ 70deb1e2 create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_next_on_master2() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;
    git.run(&["checkout", "HEAD^"])?;

    {
        let (stdout, _stderr) = git.run(&["next"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 70deb1e28791d8e7dd5a1f0c871a51b91282562f
        :
        O 62fc20d2 (master) create test1.txt
        |
        o 96d1c37a create test2.txt
        |
        @ 70deb1e2 create test3.txt
        "###);
    }

    Ok(())
}

#[test]
#[cfg(unix)]
fn test_checkout_pty() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "master"])?;
    git.detach_head()?;
    git.commit_file("test3", 3)?;

    run_in_pty(
        &git,
        &["branchless", "checkout"],
        &[
            PtyAction::WaitUntilContains("> "),
            PtyAction::Write("test1"),
            PtyAction::WaitUntilContains("> test1"),
            PtyAction::WaitUntilContains("> 62fc20d2"),
            PtyAction::Write(CARRIAGE_RETURN),
        ],
    )?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 (master) create initial.txt
        |\
        | @ 62fc20d2 create test1.txt
        | |
        | o 96d1c37a create test2.txt
        |
        o 98b9119d create test3.txt
        "###);
    }

    run_in_pty(
        &git,
        &["branchless", "checkout"],
        &[
            PtyAction::WaitUntilContains("> "),
            PtyAction::Write("test3"),
            PtyAction::WaitUntilContains("> test3"),
            PtyAction::WaitUntilContains("> 98b9119d"),
            PtyAction::Write(CARRIAGE_RETURN),
        ],
    )?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 (master) create initial.txt
        |\
        | o 62fc20d2 create test1.txt
        | |
        | o 96d1c37a create test2.txt
        |
        @ 98b9119d create test3.txt
        "###);
    }

    Ok(())
}

#[test]
#[cfg(unix)]
fn test_checkout_abort() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 1)?;

    run_in_pty(
        &git,
        &["branchless", "checkout"],
        &[
            PtyAction::WaitUntilContains("> ".into()),
            PtyAction::Write(END_OF_TEXT.into()),
        ],
    )?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        @ 142901d5 create test2.txt
        "###);
    }

    Ok(())
}

enum PtyAction<'a> {
    Write(&'a str),
    WaitUntilContains(&'a str),
}

fn run_in_pty(git: &Git, args: &[&str], inputs: &[PtyAction]) -> eyre::Result<()> {
    // Use the native pty implementation for the system
    let pty_system = native_pty_system();
    let pty_size = PtySize::default();
    let mut pty = pty_system
        .openpty(pty_size)
        .map_err(|e| eyre!("Could not open pty: {}", e))?;

    // Spawn a git instance in the pty.
    let mut cmd = CommandBuilder::new(&git.path_to_git);
    for (k, v) in git.get_base_env(0) {
        cmd.env(k, v);
    }
    cmd.env("TERM", "xterm");
    cmd.args(args);
    cmd.cwd(&git.repo_path);

    let mut child = pty
        .slave
        .spawn_command(cmd)
        .map_err(|e| eyre!("Could not spawn child: {}", e))?;
    let mut reader = pty
        .master
        .try_clone_reader()
        .map_err(|e| eyre!("Could not clone reader: {}", e))?;

    let mut parser = vt100::Parser::new(pty_size.rows, pty_size.cols, 0);
    for action in inputs {
        match &action {
            PtyAction::WaitUntilContains(value) => {
                let mut buffer = [0; 1024];
                while !parser.screen().contents().contains(value) {
                    let n = reader.read(&mut buffer)?;
                    parser.process(&buffer[..n]);
                }
            }
            PtyAction::Write(value) => {
                write!(pty.master, "{}", value)?;
                pty.master.flush()?;
            }
        }
    }

    thread::spawn(move || {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).expect("finish reading pty");
    });

    child.wait()?;

    Ok(())
}
