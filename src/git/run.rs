use std::collections::HashMap;
use std::convert::TryInto;
use std::ffi::{OsStr, OsString};
use std::fmt::Write;
use std::io::{BufRead, BufReader, Read, Write as WriteIo};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;
use eyre::{eyre, Context};
use itertools::Itertools;
use os_str_bytes::OsStrBytes;
use tracing::instrument;

use crate::commands::smartlog::smartlog;
use crate::core::config::get_core_hooks_path;
use crate::core::effects::{Effects, OperationType};
use crate::core::eventlog::{EventTransactionId, BRANCHLESS_TRANSACTION_ID_ENV_VAR};
use crate::core::formatting::printable_styled_string;
use crate::git::repo::Repo;
use crate::util::get_sh;

use super::CategorizedReferenceName;

/// Path to the `git` executable on disk to be executed.
#[derive(Clone)]
pub struct GitRunInfo {
    /// The path to the Git executable on disk.
    pub path_to_git: PathBuf,

    /// The working directory that the Git executable should be run in.
    pub working_directory: PathBuf,

    /// The environment variables that should be passed to the Git process.
    pub env: HashMap<OsString, OsString>,
}

impl std::fmt::Debug for GitRunInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "<GitRunInfo path_to_git={:?} working_directory={:?} env=not shown>",
            self.path_to_git, self.working_directory
        )
    }
}

impl GitRunInfo {
    fn spawn_writer_thread<
        InputStream: Read + Send + 'static,
        OutputStream: Write + Send + 'static,
    >(
        &self,
        stream: Option<InputStream>,
        mut output: OutputStream,
    ) -> JoinHandle<()> {
        thread::spawn(move || {
            let stream = match stream {
                Some(stream) => stream,
                None => return,
            };
            let reader = BufReader::new(stream);
            for line in reader.lines() {
                let line = line.expect("Reading line from subprocess");
                writeln!(output, "{}", line).expect("Writing line from subprocess");
            }
        })
    }

    fn run_inner(
        &self,
        effects: &Effects,
        event_tx_id: Option<EventTransactionId>,
        args: &[&OsStr],
    ) -> eyre::Result<isize> {
        let GitRunInfo {
            path_to_git,
            working_directory,
            env,
        } = self;

        let args_string = args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect_vec()
            .join(" ");
        let command_string = format!("git {}", args_string);
        let (effects, _progress) =
            effects.start_operation(OperationType::RunGitCommand(Arc::new(command_string)));
        writeln!(
            effects.get_output_stream(),
            "branchless: running command: {} {}",
            &path_to_git.to_string_lossy(),
            &args_string
        )?;

        let mut command = Command::new(path_to_git);
        command.current_dir(working_directory);
        command.args(args);
        command.env_clear();
        command.envs(env.iter());
        if let Some(event_tx_id) = event_tx_id {
            command.env(BRANCHLESS_TRANSACTION_ID_ENV_VAR, event_tx_id.to_string());
        }
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let mut child = command.spawn().wrap_err("Spawning Git subprocess")?;

        let stdout = child.stdout.take();
        let stdout_thread = self.spawn_writer_thread(stdout, effects.get_output_stream());
        let stderr = child.stderr.take();
        let stderr_thread = self.spawn_writer_thread(stderr, effects.get_error_stream());

        let exit_status = child
            .wait()
            .wrap_err("Waiting for Git subprocess to complete")?;
        stdout_thread.join().unwrap();
        stderr_thread.join().unwrap();

        // On Unix, if the child process was terminated by a signal, we need to call
        // some Unix-specific functions to access the signal that terminated it. For
        // simplicity, just return `1` in those cases.
        let exit_code = exit_status.code().unwrap_or(1);
        let exit_code = exit_code
            .try_into()
            .wrap_err("Converting exit code from i32 to isize")?;
        Ok(exit_code)
    }

    /// Run Git in a subprocess, and inform the user.
    ///
    /// This is suitable for commands which affect the working copy or should run
    /// hooks. We don't want our process to be responsible for that.
    ///
    /// `args` contains the list of arguments to pass to Git, not including the Git
    /// executable itself.
    ///
    /// Returns the exit code of Git (non-zero signifies error).
    #[instrument]
    #[must_use = "The return code for `run_git` must be checked"]
    pub fn run<S: AsRef<OsStr> + std::fmt::Debug>(
        &self,
        effects: &Effects,
        event_tx_id: Option<EventTransactionId>,
        args: &[S],
    ) -> eyre::Result<isize> {
        self.run_inner(
            effects,
            event_tx_id,
            args.iter().map(AsRef::as_ref).collect_vec().as_slice(),
        )
    }

    fn run_silent_inner(
        &self,
        repo: &Repo,
        event_tx_id: Option<EventTransactionId>,
        args: &[&str],
    ) -> eyre::Result<String> {
        let GitRunInfo {
            path_to_git,
            working_directory,
            env,
        } = self;

        // Technically speaking, we should be able to work with non-UTF-8 repository
        // paths. Need to make the typechecker accept it.
        let repo_path = repo.get_path();
        let repo_path = repo_path.to_str().ok_or_else(|| {
            eyre::eyre!(
                "Path to Git repo could not be converted to UTF-8 string: {:?}",
                repo_path
            )
        })?;

        let args = {
            let mut result = vec!["-C", repo_path];
            result.extend(args);
            result
        };
        let mut command = Command::new(path_to_git);
        command.args(&args);
        command.current_dir(working_directory);
        command.env_clear();
        command.envs(env.iter());
        if let Some(event_tx_id) = event_tx_id {
            command.env(BRANCHLESS_TRANSACTION_ID_ENV_VAR, event_tx_id.to_string());
        }
        let result = command.output().wrap_err("Spawning Git subprocess")?;
        let result =
            String::from_utf8(result.stdout).wrap_err("Decoding stdout from Git subprocess")?;
        Ok(result)
    }

    /// Run Git silently (don't display output to the user).
    ///
    /// Whenever possible, use `git2`'s bindings to Git instead, as they're
    /// considerably more lightweight and reliable.
    ///
    /// Returns the stdout of the Git invocation.
    pub fn run_silent<S: AsRef<str> + std::fmt::Debug>(
        &self,
        repo: &Repo,
        event_tx_id: Option<EventTransactionId>,
        args: &[S],
    ) -> eyre::Result<String> {
        self.run_silent_inner(
            repo,
            event_tx_id,
            args.iter().map(AsRef::as_ref).collect_vec().as_slice(),
        )
    }

    fn run_hook_inner(
        &self,
        effects: &Effects,
        repo: &Repo,
        hook_name: &str,
        event_tx_id: EventTransactionId,
        args: &[&str],
        stdin: Option<OsString>,
    ) -> eyre::Result<()> {
        let hook_dir = get_core_hooks_path(repo)?;
        if !hook_dir.exists() {
            return Ok(());
        }

        let GitRunInfo {
            // We're calling a Git hook, but not Git itself.
            path_to_git: _,
            // We always want to call the hook in the Git working copy,
            // regardless of where the Git executable was invoked.
            working_directory: _,
            env,
        } = self;
        let path = {
            let mut path_components: Vec<PathBuf> =
                vec![std::fs::canonicalize(&hook_dir).wrap_err("Canonicalizing hook dir")?];
            if let Some(path) = env.get(OsStr::new("PATH")) {
                path_components.extend(std::env::split_paths(path));
            }
            std::env::join_paths(path_components).wrap_err("Joining path components")?
        };

        if hook_dir.join(hook_name).exists() {
            let mut child = Command::new(get_sh().ok_or_else(|| eyre!("could not get sh"))?)
                // From `githooks(5)`: Before Git invokes a hook, it changes its
                // working directory to either $GIT_DIR in a bare repository or the
                // root of the working tree in a non-bare repository.
                .current_dir(
                    repo.get_working_copy_path()
                        .unwrap_or_else(|| repo.get_path()),
                )
                .arg("-c")
                .arg(format!("{} \"$@\"", hook_name))
                .arg(hook_name) // "$@" expands "$1" "$2" "$3" ... but we also must specify $0.
                .args(args)
                .env_clear()
                .envs(env.iter())
                .env(BRANCHLESS_TRANSACTION_ID_ENV_VAR, event_tx_id.to_string())
                .env("PATH", &path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .wrap_err_with(|| format!("Invoking {} hook with PATH: {:?}", &hook_name, &path))?;

            if let Some(stdin) = stdin {
                child
                    .stdin
                    .as_mut()
                    .unwrap()
                    .write_all(&stdin.to_raw_bytes())
                    .wrap_err("Writing hook process stdin")?;
            }

            let stdout = child.stdout.take();
            let stdout_thread = self.spawn_writer_thread(stdout, effects.get_output_stream());
            let stderr = child.stderr.take();
            let stderr_thread = self.spawn_writer_thread(stderr, effects.get_error_stream());

            let _ignored: ExitStatus =
                child.wait().wrap_err("Waiting for child process to exit")?;
            stdout_thread.join().unwrap();
            stderr_thread.join().unwrap();
        }
        Ok(())
    }

    /// Run a provided Git hook if it exists for the repository.
    ///
    /// See the man page for `githooks(5)` for more detail on Git hooks.
    #[instrument]
    pub fn run_hook<S: AsRef<str> + std::fmt::Debug>(
        &self,
        effects: &Effects,
        repo: &Repo,
        hook_name: &str,
        event_tx_id: EventTransactionId,
        args: &[S],
        stdin: Option<OsString>,
    ) -> eyre::Result<()> {
        self.run_hook_inner(
            effects,
            repo,
            hook_name,
            event_tx_id,
            args.iter().map(AsRef::as_ref).collect_vec().as_slice(),
            stdin,
        )
    }
}

/// Checks out the requested commit. If the operation succeeds, then displays
/// the new smartlog. Otherwise displays a warning message.
pub fn check_out_commit(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    event_tx_id: Option<EventTransactionId>,
    target: impl AsRef<OsStr> + std::fmt::Debug,
) -> eyre::Result<isize> {
    let categorized_target = CategorizedReferenceName::new(target.as_ref());
    let target = categorized_target.remove_prefix()?;

    let result = git_run_info.run(effects, event_tx_id, &[OsStr::new("checkout"), &target])?;

    if result == 0 {
        smartlog(effects, &Default::default())?;
    } else {
        writeln!(
            effects.get_output_stream(),
            "{}",
            printable_styled_string(
                effects.get_glyphs(),
                StyledString::styled(
                    format!("Failed to check out commit {:?}", target),
                    BaseColor::Red.light()
                )
            )?
        )?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use crate::testing::make_git;

    #[test]
    fn test_hook_working_dir() -> eyre::Result<()> {
        let git = make_git()?;

        if !git.supports_reference_transactions()? {
            return Ok(());
        }

        git.init_repo()?;
        git.commit_file("test1", 1)?;

        std::fs::write(
            git.repo_path
                .join(".git")
                .join("hooks")
                .join("post-rewrite"),
            r#"#!/bin/sh
                   # This won't work unless we're running the hook in the Git working copy.
                   echo "Check if test1.txt exists"
                   [ -f test1.txt ] && echo "test1.txt exists"
                   "#,
        )?;

        {
            // Trigger the `post-rewrite` hook that we wrote above.
            let (stdout, stderr) = git.run(&["commit", "--amend", "-m", "foo"])?;
            insta::assert_snapshot!(stderr, @r###"
            branchless: processing 2 updates: branch master, ref HEAD
            branchless: processed commit: f23bf8f7 foo
            Check if test1.txt exists
            test1.txt exists
            "###);
            insta::assert_snapshot!(stdout, @r###"
                [master f23bf8f] foo
                 Date: Thu Oct 29 12:34:56 2020 -0100
                 1 file changed, 1 insertion(+)
                 create mode 100644 test1.txt
                "###);
        }

        Ok(())
    }
}
