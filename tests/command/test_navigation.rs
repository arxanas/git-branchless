use std::io::{Read, Write};
use std::thread;
use std::thread::sleep;
use std::time::Duration;

use eyre::eyre;

use branchless::testing::{make_git, GitRunOptions, GitWrapper};
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
fn test_checkout() -> eyre::Result<()> {
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
        &["test1", CARRIAGE_RETURN],
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
        &["test3", CARRIAGE_RETURN],
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

    run_in_pty(&git, &["branchless", "checkout"], &["test1", END_OF_TEXT])?;
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

fn run_in_pty(git: &GitWrapper, args: &[&str], inputs: &[&str]) -> eyre::Result<()> {
    // Use the native pty implementation for the system
    let pty_system = native_pty_system();
    let mut pty = pty_system.openpty(PtySize::default()).unwrap();

    let args: Vec<&str> = {
        let repo_path = git.repo_path.to_str().expect("Could not decode repo path");
        let mut new_args: Vec<&str> = vec!["-C", repo_path];
        new_args.extend(args);
        new_args
    };

    // Spawn a git instance in the pty.
    let mut cmd = CommandBuilder::new(&git.path_to_git);
    let time = 0;
    for (k, v) in git.get_base_env(&time) {
        cmd.env(k, v);
    }
    cmd.args(args);

    let mut child = pty
        .slave
        .spawn_command(cmd)
        .map_err(|e| eyre!("Could not spawn child: {}", e))?;
    let mut reader = pty
        .master
        .try_clone_reader()
        .map_err(|e| eyre!("Could not clone reader: {}", e))?;

    thread::spawn(move || loop {
        let mut s = String::new();
        reader.read_to_string(&mut s).unwrap();
    });

    for input in inputs {
        // Sleep between inputs, to give the pty time to catch up.
        sleep(Duration::from_millis(100));
        write!(pty.master, "{}", input)?;
        pty.master.flush()?;
    }

    child.wait()?;

    Ok(())
}
