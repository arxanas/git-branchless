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
        branchless: running command: <git-executable> checkout f777ecc9b0db5ed372b2615695191a8a17f79f24
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
        insta::assert_snapshot!(stdout, @"No more parent commits to go to after traversing 0 parents.
");
        insta::assert_snapshot!(stderr, @"");
    }

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        @ f777ecc9 create initial.txt
        |
        O 62fc20d2 (master) create test1.txt
        "###);
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
        branchless: running command: <git-executable> checkout f777ecc9b0db5ed372b2615695191a8a17f79f24
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
        Found multiple possible child commits to go to after traversing 0 children:
          - 62fc20d2 create test1.txt (oldest)
          - fe65c1fe create test2.txt
          - 98b9119d create test3.txt (newest)
        (Pass --oldest (-o), --newest (-n), or --interactive (-i) to select between ambiguous commits)
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
#[cfg(unix)]
fn test_next_ambiguous_interactive() -> eyre::Result<()> {
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

    run_in_pty(
        &git,
        &["next", "--interactive"],
        &[
            PtyAction::WaitUntilContains("> "),
            PtyAction::Write("test2"),
            PtyAction::WaitUntilContains("> test2"),
            PtyAction::WaitUntilContains("> fe65c1fe"),
            PtyAction::Write(CARRIAGE_RETURN),
        ],
    )?;

    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 (master) create initial.txt
        |\
        | o 62fc20d2 create test1.txt
        |\
        | @ fe65c1fe create test2.txt
        |
        o 98b9119d create test3.txt
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
fn test_next_no_more_children() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.detach_head()?;
    git.commit_file("test3", 3)?;
    git.run(&["checkout", "HEAD^^"])?;

    {
        let (stdout, _stderr) = git.run(&["next", "3"])?;
        insta::assert_snapshot!(stdout, @r###"
        No more child commits to go to after traversing 2 children.
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
    cmd.env_clear();
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
        match action {
            PtyAction::WaitUntilContains(value) => {
                while !parser.screen().contents().contains(value) {
                    const BUF_SIZE: usize = 4096;
                    let mut buffer = [0; BUF_SIZE];
                    let n = reader.read(&mut buffer)?;
                    assert!(n < BUF_SIZE, "filled up PTY buffer by reading {} bytes", n);
                    parser.process(&buffer[..n]);
                }
            }

            PtyAction::Write(value) => {
                write!(pty.master, "{}", value)?;
                pty.master.flush()?;
            }
        }
    }

    let read_remainder_of_pty_output_thread = thread::spawn(move || {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).expect("finish reading pty");
        String::from_utf8(buffer).unwrap()
    });

    child.wait()?;
    let remainder_of_pty_output = read_remainder_of_pty_output_thread.join().unwrap();
    assert!(
        !remainder_of_pty_output.contains("panic"),
        "Panic in PTY thread:\n{}",
        console::strip_ansi_codes(&remainder_of_pty_output)
    );

    Ok(())
}

#[test]
fn test_navigation_traverse_all_the_way() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;

    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.commit_file("test3", 3)?;

    {
        let (stdout, _stderr) = git.run(&["prev", "-a"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
        O f777ecc9 (master) create initial.txt
        |
        @ 62fc20d2 create test1.txt
        |
        o 96d1c37a create test2.txt
        |
        o 70deb1e2 create test3.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["next", "-a"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 70deb1e28791d8e7dd5a1f0c871a51b91282562f
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        o 96d1c37a create test2.txt
        |
        @ 70deb1e2 create test3.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_navigation_traverse_branches() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["branch", "foo"])?;
    git.commit_file("test3", 3)?;
    git.commit_file("test4", 4)?;
    git.run(&["branch", "bar"])?;
    git.commit_file("test5", 5)?;

    {
        let (stdout, _stderr) = git.run(&["prev", "-b", "2"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout foo
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        @ 96d1c37a (foo) create test2.txt
        |
        o 70deb1e2 create test3.txt
        |
        o 355e173b (bar) create test4.txt
        |
        o f81d55c0 create test5.txt
        "###);

        let (stdout, _stderr) = git.run(&["prev", "-a", "-b"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout foo
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        @ 96d1c37a (foo) create test2.txt
        |
        o 70deb1e2 create test3.txt
        |
        o 355e173b (bar) create test4.txt
        |
        o f81d55c0 create test5.txt
        "###);

        let (stdout, _stderr) = git.run(&["prev", "-b"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout master
        @ f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        o 96d1c37a (foo) create test2.txt
        |
        o 70deb1e2 create test3.txt
        |
        o 355e173b (bar) create test4.txt
        |
        o f81d55c0 create test5.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["next", "-b", "2"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout bar
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        o 96d1c37a (foo) create test2.txt
        |
        o 70deb1e2 create test3.txt
        |
        @ 355e173b (bar) create test4.txt
        |
        o f81d55c0 create test5.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["next", "-a", "-b"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout bar
        O f777ecc9 (master) create initial.txt
        |
        o 62fc20d2 create test1.txt
        |
        o 96d1c37a (foo) create test2.txt
        |
        o 70deb1e2 create test3.txt
        |
        @ 355e173b (bar) create test4.txt
        |
        o f81d55c0 create test5.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_traverse_branches_ambiguous() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.run(&["branch", "foo"])?;
    git.run(&["branch", "bar"])?;
    git.commit_file("test2", 2)?;

    {
        let (stdout, _stderr) = git.run(&["prev", "--all", "--branch"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 62fc20d2a290daea0d52bdc2ed2ad4be6491010e
        O f777ecc9 (master) create initial.txt
        |
        @ 62fc20d2 (bar, foo) create test1.txt
        |
        o 96d1c37a create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_navigation_failed_to_check_out_commit() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "HEAD^"])?;

    git.write_file("test2", "conflicting contents")?;

    {
        let (stdout, _stderr) = git.run_with_options(
            &["next"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 96d1c37a3d4363611c49f7e52186e189a04c531f
        Failed to check out commit: 96d1c37a3d4363611c49f7e52186e189a04c531f
        "###);
    }

    Ok(())
}

#[test]
#[cfg(unix)]
fn test_checkout_pty_branch() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    let test1_oid = git.commit_file("test1", 1)?;
    git.detach_head()?;
    git.commit_file("test2", 2)?;
    git.run(&["checkout", "--detach", "master"])?;
    git.commit_file("test3", 3)?;

    run_in_pty(
        &git,
        &["branchless", "checkout"],
        &[
            PtyAction::WaitUntilContains("> "),
            PtyAction::Write("master"),
            PtyAction::WaitUntilContains(&format!("> {}", &test1_oid.to_string()[0..6])),
            PtyAction::Write(CARRIAGE_RETURN),
        ],
    )?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        :
        @ 62fc20d2 (master) create test1.txt
        |\
        | o 96d1c37a create test2.txt
        |
        o 4838e49b create test3.txt
        "###);
    }

    Ok(())
}

#[test]
#[cfg(unix)]
fn test_checkout_pty_initial_query() -> eyre::Result<()> {
    let git = make_git()?;

    git.init_repo()?;
    git.detach_head()?;
    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    run_in_pty(
        &git,
        &["branchless", "checkout", "-i", "test1"],
        &[
            PtyAction::WaitUntilContains("> test1"),
            PtyAction::Write(CARRIAGE_RETURN),
        ],
    )?;
    {
        let (stdout, _stderr) = git.run(&["smartlog"])?;
        insta::assert_snapshot!(stdout, @r###"
        O f777ecc9 (master) create initial.txt
        |
        @ 62fc20d2 create test1.txt
        |
        o 96d1c37a create test2.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_navigation_merge() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file_with_contents("conflicting", 1, "foo\nbar\n")?;
    git.commit_file_with_contents("conflicting", 2, "baz\nqux\n")?;
    git.write_file("conflicting", "foo\nbar\nqux\n")?;

    {
        let (stdout, stderr) = git.run_with_options(
            &["prev"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 25497cb08387d7d20aa741398b73ce7f924afdb5
        Failed to check out commit: 25497cb08387d7d20aa741398b73ce7f924afdb5
        "###);
        insta::assert_snapshot!(stderr, @r###"
        error: Your local changes to the following files would be overwritten by checkout:
        	conflicting.txt
        Please commit your changes or stash them before you switch branches.
        Aborting
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["prev", "--merge"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 25497cb08387d7d20aa741398b73ce7f924afdb5 --merge
        M	conflicting.txt
        O f777ecc9 (master) create initial.txt
        |
        @ 25497cb0 create conflicting.txt
        |
        o 6dd50913 create conflicting.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["diff"])?;
        insta::assert_snapshot!(stdout, @r###"
        diff --cc conflicting.txt
        index 3bd1f0e,72594ed..0000000
        --- a/conflicting.txt
        +++ b/conflicting.txt
        @@@ -1,2 -1,3 +1,6 @@@
          foo
          bar
        ++<<<<<<< 25497cb08387d7d20aa741398b73ce7f924afdb5
        ++=======
        + qux
        ++>>>>>>> local
        "###);
    }

    Ok(())
}

#[test]
fn test_navigation_force() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.detach_head()?;
    git.commit_file_with_contents("conflicting", 1, "foo\nbar\n")?;
    git.commit_file_with_contents("conflicting", 2, "baz\nqux\n")?;
    git.write_file("conflicting", "foo\n\nbar\nqux\n")?;

    {
        let (stdout, stderr) = git.run_with_options(
            &["prev"],
            &GitRunOptions {
                expected_exit_code: 1,
                ..Default::default()
            },
        )?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 25497cb08387d7d20aa741398b73ce7f924afdb5
        Failed to check out commit: 25497cb08387d7d20aa741398b73ce7f924afdb5
        "###);
        insta::assert_snapshot!(stderr, @r###"
        error: Your local changes to the following files would be overwritten by checkout:
        	conflicting.txt
        Please commit your changes or stash them before you switch branches.
        Aborting
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["prev", "--force"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout 25497cb08387d7d20aa741398b73ce7f924afdb5 --force
        O f777ecc9 (master) create initial.txt
        |
        @ 25497cb0 create conflicting.txt
        |
        o 6dd50913 create conflicting.txt
        "###);
    }

    {
        let (stdout, _stderr) = git.run(&["diff"])?;
        insta::assert_snapshot!(stdout, @"");
    }

    Ok(())
}

#[test]
fn test_navigation_checkout_flags() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    git.commit_file("test1", 1)?;
    git.commit_file("test2", 2)?;

    {
        let git = git.duplicate_repo()?;
        {
            let (stdout, _stderr) = git.run(&["branchless", "checkout", "-b", "foo", "HEAD^"])?;
            insta::assert_snapshot!(stdout, @r###"
            branchless: running command: <git-executable> checkout HEAD^ -b foo
            :
            @ 62fc20d2 (foo) create test1.txt
            |
            O 96d1c37a (master) create test2.txt
            "###);
        }

        {
            let (stdout, _stderr) = git.run(&["branchless", "checkout", "-b", "bar"])?;
            insta::assert_snapshot!(stdout, @r###"
            branchless: running command: <git-executable> checkout -b bar
            :
            @ 62fc20d2 (bar, foo) create test1.txt
            |
            O 96d1c37a (master) create test2.txt
            "###);
        }
    }

    {
        let git = git.duplicate_repo()?;
        {
            git.commit_file_with_contents("test1", 3, "new contents")?;
            git.write_file("test1", "conflicting\n")?;
            let (stdout, _stderr) = git.run(&["branchless", "checkout", "-f", "HEAD~2"])?;
            insta::assert_snapshot!(stdout, @r###"
            branchless: running command: <git-executable> checkout HEAD~2 -f
            :
            @ 62fc20d2 create test1.txt
            :
            O 5b738c1c (master) create test1.txt
            "###);
        }
    }

    {
        git.commit_file_with_contents("test1", 3, "new contents")?;
        git.write_file("test1", "conflicting\n")?;
        let (stdout, _stderr) = git.run(&["branchless", "checkout", "-m", "HEAD~2"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout HEAD~2 -m
        M	test1.txt
        :
        @ 62fc20d2 create test1.txt
        :
        O 5b738c1c (master) create test1.txt
        "###);
    }

    Ok(())
}

#[test]
fn test_navigation_checkout_target_only() -> eyre::Result<()> {
    let git = make_git()?;
    git.init_repo()?;

    {
        let (stdout, _stderr) = git.run(&["branchless", "checkout", "master"])?;
        insta::assert_snapshot!(stdout, @r###"
        branchless: running command: <git-executable> checkout master
        @ f777ecc9 (master) create initial.txt
        "###);
    }

    Ok(())
}
