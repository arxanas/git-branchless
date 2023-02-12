use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use eyre::eyre;
use lib::testing::Git;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

pub enum PtyAction<'a> {
    Write(&'a str),
    WaitUntilContains(&'a str),
}

pub fn run_in_pty(
    git: &Git,
    branchless_subcommand: &str,
    args: &[&str],
    inputs: &[PtyAction],
) -> eyre::Result<()> {
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
    cmd.arg("branchless");
    cmd.arg(branchless_subcommand);
    cmd.args(args);
    cmd.cwd(&git.repo_path);

    let mut child = pty
        .slave
        .spawn_command(cmd)
        .map_err(|e| eyre!("Could not spawn child: {}", e))?;

    let reader = pty
        .master
        .try_clone_reader()
        .map_err(|e| eyre!("Could not clone reader: {}", e))?;
    let reader = Arc::new(Mutex::new(reader));

    let parser = vt100::Parser::new(pty_size.rows, pty_size.cols, 0);
    let parser = Arc::new(Mutex::new(parser));

    for action in inputs {
        match action {
            PtyAction::WaitUntilContains(value) => {
                let (finished_tx, finished_rx) = channel();

                let wait_thread = {
                    let parser = Arc::clone(&parser);
                    let reader = Arc::clone(&reader);
                    let value = value.to_string();
                    thread::spawn(move || -> anyhow::Result<()> {
                        loop {
                            // Drop the `parser` lock after this, since we may block
                            // on `reader.read` below, and the caller may want to
                            // check the screen contents of `parser`.
                            {
                                let parser = parser.lock().unwrap();
                                if parser.screen().contents().contains(&value) {
                                    break;
                                }
                            }

                            let mut reader = reader.lock().unwrap();
                            const BUF_SIZE: usize = 4096;
                            let mut buffer = [0; BUF_SIZE];
                            let n = reader.read(&mut buffer)?;
                            assert!(n < BUF_SIZE, "filled up PTY buffer by reading {} bytes", n);

                            {
                                let mut parser = parser.lock().unwrap();
                                parser.process(&buffer[..n]);
                            }
                        }

                        finished_tx.send(()).unwrap();
                        Ok(())
                    })
                };

                if finished_rx.recv_timeout(Duration::from_secs(5)).is_err() {
                    panic!(
                        "\
Timed out waiting for virtual terminal to show string: {:?}
Screen contents:
-----
{}
-----
",
                        value,
                        parser.lock().unwrap().screen().contents(),
                    );
                }

                wait_thread.join().unwrap().unwrap();
            }

            PtyAction::Write(value) => {
                write!(pty.master, "{value}")?;
                pty.master.flush()?;
            }
        }
    }

    let read_remainder_of_pty_output_thread = thread::spawn({
        let reader = Arc::clone(&reader);
        move || {
            let mut reader = reader.lock().unwrap();
            let mut buffer = Vec::new();
            reader.read_to_end(&mut buffer).expect("finish reading pty");
            String::from_utf8(buffer).unwrap()
        }
    });
    child.wait()?;

    let _ = read_remainder_of_pty_output_thread;
    // Useful for debugging, but seems to deadlock on some tests:
    // let remainder_of_pty_output = read_remainder_of_pty_output_thread.join().unwrap();
    // assert!(
    //     !remainder_of_pty_output.contains("panic"),
    //     "Panic in PTY thread:\n{}",
    //     console::strip_ansi_codes(&remainder_of_pty_output)
    // );

    Ok(())
}
