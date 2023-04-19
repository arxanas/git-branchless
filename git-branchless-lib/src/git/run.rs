use std::collections::HashMap;
use std::convert::TryInto;
use std::ffi::{OsStr, OsString};
use std::fmt::Write;
use std::io::{BufRead, BufReader, Read, Write as WriteIo};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use bstr::BString;
use eyre::{eyre, Context};
use itertools::Itertools;
use tracing::instrument;

use crate::core::config::get_hooks_dir;
use crate::core::effects::{Effects, OperationType};
use crate::core::eventlog::{EventTransactionId, BRANCHLESS_TRANSACTION_ID_ENV_VAR};
use crate::git::repo::Repo;
use crate::util::{get_sh, ExitCode, EyreExitOr};

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

/// Options for invoking Git.
pub struct GitRunOpts {
    /// If set, a non-zero exit code will be treated as an error.
    pub treat_git_failure_as_error: bool,

    /// A vector of bytes to write to the Git process's stdin. If `None`,
    /// nothing is written to stdin.
    pub stdin: Option<Vec<u8>>,
}

impl Default for GitRunOpts {
    fn default() -> Self {
        Self {
            treat_git_failure_as_error: true,
            stdin: None,
        }
    }
}

/// The result of invoking Git.
#[must_use]
pub struct GitRunResult {
    /// The exit code of the process.
    pub exit_code: ExitCode,

    /// The stdout contents written by the invocation.
    pub stdout: Vec<u8>,

    /// The stderr contents written by the invocation.
    pub stderr: Vec<u8>,
}

impl std::fmt::Debug for GitRunResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "<GitRunResult exit_code={:?} stdout={:?} stderr={:?}>",
            self.exit_code,
            String::from_utf8_lossy(&self.stdout),
            String::from_utf8_lossy(&self.stderr),
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
                writeln!(output, "{line}").expect("Writing line from subprocess");
            }
        })
    }

    fn run_inner(
        &self,
        effects: &Effects,
        event_tx_id: Option<EventTransactionId>,
        args: &[&OsStr],
    ) -> EyreExitOr<()> {
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
        let command_string = format!("git {args_string}");
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
        let exit_code: i32 = exit_status.code().unwrap_or(1);
        let exit_code: isize = exit_code
            .try_into()
            .wrap_err("Converting exit code from i32 to isize")?;
        let exit_code = ExitCode(exit_code);
        if exit_code.is_success() {
            Ok(Ok(()))
        } else {
            Ok(Err(exit_code))
        }
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
    #[must_use = "The return code for `GitRunInfo::run` must be checked"]
    pub fn run<S: AsRef<OsStr> + std::fmt::Debug>(
        &self,
        effects: &Effects,
        event_tx_id: Option<EventTransactionId>,
        args: &[S],
    ) -> EyreExitOr<()> {
        self.run_inner(
            effects,
            event_tx_id,
            args.iter().map(AsRef::as_ref).collect_vec().as_slice(),
        )
    }

    /// Run the provided command without wrapping it in an `Effects` operation.
    /// This may clobber progress reporting, and is usually not what you want;
    /// see [`GitRunInfo::run`] instead.
    #[instrument]
    #[must_use = "The return code for `GitRunInfo::run_direct_no_wrapping` must be checked"]
    pub fn run_direct_no_wrapping(
        &self,
        event_tx_id: Option<EventTransactionId>,
        args: &[impl AsRef<OsStr> + std::fmt::Debug],
    ) -> EyreExitOr<()> {
        let GitRunInfo {
            path_to_git,
            working_directory,
            env,
        } = self;

        let mut command = Command::new(path_to_git);
        command.current_dir(working_directory);
        command.args(args);
        command.env_clear();
        command.envs(env.iter());
        if let Some(event_tx_id) = event_tx_id {
            command.env(BRANCHLESS_TRANSACTION_ID_ENV_VAR, event_tx_id.to_string());
        }

        let mut child = command.spawn().wrap_err("Spawning Git subprocess")?;
        let exit_status = child
            .wait()
            .wrap_err("Waiting for Git subprocess to complete")?;

        // On Unix, if the child process was terminated by a signal, we need to call
        // some Unix-specific functions to access the signal that terminated it. For
        // simplicity, just return `1` in those cases.
        let exit_code: i32 = exit_status.code().unwrap_or(1);
        let exit_code: isize = exit_code
            .try_into()
            .wrap_err("Converting exit code from i32 to isize")?;
        let exit_code = ExitCode(exit_code);
        if exit_code.is_success() {
            Ok(Ok(()))
        } else {
            Ok(Err(exit_code))
        }
    }

    /// Returns the working directory for commands run on a given `Repo`.
    ///
    /// This is typically the working copy path for the repo.
    ///
    /// Some commands (notably `git status`) do not function correctly when run
    /// from the git repo (i.e. `.git`) path.
    /// Hooks should also be run from the working copy path - from
    /// `githooks(5)`: Before Git invokes a hook, it changes its working
    /// directory to either $GIT_DIR in a bare repository or the root of the
    /// working tree in a non-bare repository.
    ///
    /// This contains additional logic for symlinked `.git` directories to work
    /// around an upstream `libgit2` issue.
    fn working_directory<'a>(&'a self, repo: &'a Repo) -> &'a Path {
        repo.get_working_copy_path()
            // `libgit2` returns the working copy path as the parent of the
            // `.git` directory. However, if the `.git` directory is a symlink,
            // `libgit2` *resolves the symlink*, returning an incorrect working
            // directory: https://github.com/libgit2/libgit2/issues/6401
            //
            // Detect this case by checking if the current working directory is
            // not a subdirectory of the returned working copy. While this does
            // not work if the symlinked `.git` directory points to a child of
            // an ancestor of the directory, this should handle most cases,
            // including working copies managed by the `repo` tool.
            //
            // This workaround may result in slightly incorrect hook behavior,
            // as the current working directory may be a subdirectory of the
            // root of the working tree.
            .map(|working_copy| {
                // Both paths are already "canonicalized".
                if !self.working_directory.starts_with(working_copy) {
                    &self.working_directory
                } else {
                    working_copy
                }
            })
            .unwrap_or_else(|| repo.get_path())
    }

    fn run_silent_inner(
        &self,
        repo: &Repo,
        event_tx_id: Option<EventTransactionId>,
        args: &[&str],
        opts: GitRunOpts,
    ) -> eyre::Result<GitRunResult> {
        let GitRunInfo {
            path_to_git,
            working_directory,
            env,
        } = self;
        let GitRunOpts {
            treat_git_failure_as_error,
            stdin,
        } = opts;

        let repo_path = self.working_directory(repo);
        // Technically speaking, we should be able to work with non-UTF-8 repository
        // paths. Need to make the typechecker accept it.
        let repo_path = repo_path.to_str().ok_or_else(|| {
            eyre::eyre!(
                "Path to Git repo could not be converted to UTF-8 string: {:?}",
                repo.get_path()
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

        command.stdin(match stdin {
            Some(_) => Stdio::piped(),
            None => Stdio::null(),
        });
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        let mut child = command.spawn().wrap_err("Spawning Git subprocess")?;

        if let Some(stdin) = stdin {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(&stdin)
                .wrap_err("Writing process stdin")?;
        }

        let output = child
            .wait_with_output()
            .wrap_err("Spawning Git subprocess")?;
        let exit_code = ExitCode(output.status.code().unwrap_or(1).try_into()?);
        let result = GitRunResult {
            // On Unix, if the child process was terminated by a signal, we need to call
            // some Unix-specific functions to access the signal that terminated it. For
            // simplicity, just return `1` in those cases.
            exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
        };
        if treat_git_failure_as_error && !exit_code.is_success() {
            eyre::bail!(
                "Git subprocess failed:\nArgs: {:?}\nResult: {:?}",
                &args,
                result
            );
        }
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
        opts: GitRunOpts,
    ) -> eyre::Result<GitRunResult> {
        self.run_silent_inner(
            repo,
            event_tx_id,
            args.iter().map(AsRef::as_ref).collect_vec().as_slice(),
            opts,
        )
    }

    fn run_hook_inner(
        &self,
        effects: &Effects,
        repo: &Repo,
        hook_name: &str,
        event_tx_id: EventTransactionId,
        args: &[&str],
        stdin: Option<BString>,
    ) -> eyre::Result<()> {
        let hook_dir = get_hooks_dir(self, repo, Some(event_tx_id))?;
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
                .current_dir(self.working_directory(repo))
                .arg("-c")
                .arg(format!("{hook_name} \"$@\""))
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
                    .write_all(&stdin)
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
        stdin: Option<BString>,
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
