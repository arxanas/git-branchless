//! Testing utilities.
//!
//! This is inside `src` rather than `tests` since we use this code in some unit
//! tests.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::core::config::env_vars::{
    get_git_exec_path, get_path_to_git, should_use_separate_command_binary, TEST_GIT,
    TEST_SEPARATE_COMMAND_BINARIES,
};
use crate::git::{GitRunInfo, GitVersion, NonZeroOid, Repo};
use crate::util::get_sh;
use color_eyre::Help;
use eyre::Context;
use itertools::Itertools;
use lazy_static::lazy_static;
use once_cell::sync::OnceCell;
use regex::{Captures, Regex};
use tempfile::TempDir;
use tracing::{instrument, warn};

const DUMMY_NAME: &str = "Testy McTestface";
const DUMMY_EMAIL: &str = "test@example.com";
const DUMMY_DATE: &str = "Wed 29 Oct 12:34:56 2020 PDT";

/// Wrapper around the Git executable, for testing.
#[derive(Clone, Debug)]
pub struct Git {
    /// The path to the repository on disk. The directory itself must exist,
    /// although it might not have a `.git` folder in it. (Use `Git::init_repo`
    /// to initialize it.)
    pub repo_path: PathBuf,

    /// The path to the Git executable on disk. This is important since we test
    /// against multiple Git versions.
    pub path_to_git: PathBuf,

    /// The `GIT_EXEC_PATH` environment variable value to use for testing.
    pub git_exec_path: PathBuf,
}

/// Options for `Git::init_repo_with_options`.
#[derive(Debug)]
pub struct GitInitOptions {
    /// If `true`, then `init_repo_with_options` makes an initial commit with
    /// some content.
    pub make_initial_commit: bool,

    /// If `true`, run `git branchless init` as part of initialization process.
    pub run_branchless_init: bool,
}

impl Default for GitInitOptions {
    fn default() -> Self {
        GitInitOptions {
            make_initial_commit: true,
            run_branchless_init: true,
        }
    }
}

/// Options for `Git::run_with_options`.
#[derive(Debug, Default)]
pub struct GitRunOptions {
    /// The timestamp of the command. Mostly useful for `git commit`. This should
    /// be a number like 0, 1, 2, 3...
    pub time: isize,

    /// The exit code that `Git` should return.
    pub expected_exit_code: i32,

    /// The input to write to the child process's stdin.
    pub input: Option<String>,

    /// Additional environment variables to start the process with.
    pub env: HashMap<String, String>,
}

impl Git {
    /// Constructor.
    pub fn new(path_to_git: PathBuf, repo_path: PathBuf, git_exec_path: PathBuf) -> Self {
        Git {
            repo_path,
            path_to_git,
            git_exec_path,
        }
    }

    /// Replace dynamic strings in the output, for testing purposes.
    pub fn preprocess_output(&self, stdout: String) -> eyre::Result<String> {
        let path_to_git = self
            .path_to_git
            .to_str()
            .ok_or_else(|| eyre::eyre!("Could not convert path to Git to string"))?;
        let output = stdout.replace(path_to_git, "<git-executable>");

        // NB: tests which run on Windows are unlikely to succeed due to this
        // `canonicalize` call.
        let repo_path = std::fs::canonicalize(&self.repo_path)?;

        let repo_path = repo_path
            .to_str()
            .ok_or_else(|| eyre::eyre!("Could not convert repo path to string"))?;
        let output = output.replace(repo_path, "<repo-path>");

        lazy_static! {
            // Simulate clearing the terminal line by searching for the
            // appropriate sequences of characters and removing the line
            // preceding them.
            //
            // - `\r`: Interactive progress displays may update the same line
            // multiple times with a carriage return before emitting the final
            // newline.
            // - `\x1B[K`: Window pseudo console may emit EL 'Erase in Line' VT
            // sequences.
            static ref CLEAR_LINE_RE: Regex = Regex::new(r"(^|\n).*(\r|\x1B\[K)").unwrap();
        }
        let output = CLEAR_LINE_RE
            .replace_all(&output, |captures: &Captures| {
                // Restore the leading newline, if any.
                captures[1].to_string()
            })
            .into_owned();

        lazy_static! {
            // Convert non-empty, blank lines to empty, blank lines to make the
            // command output easier to use in snapshot assertions.
            //
            // Some git output includes lines made up of only whitespace, which
            // is hard to work with as inline snapshots because editors often
            // trim such trailing whitespace automatically. In particular,
            // `git show` prints 4 spaces instead of just an empty line for the
            // lines between commit message paragraphs. (ie "    \n" instead of
            // just "\n".)
            //
            // This regex targets the output of `git show`, but it could be
            // generalized in the future if other cases come up.
            static ref WHITESPACE_ONLY_LINE_RE: Regex = Regex::new(r"(?m)^ {4}$").unwrap();
        }
        let output = WHITESPACE_ONLY_LINE_RE
            .replace_all(&output, "")
            .into_owned();

        Ok(output)
    }

    /// Get the `PATH` environment variable to use for testing.
    pub fn get_path_for_env(&self) -> OsString {
        let cargo_bin_path = assert_cmd::cargo::cargo_bin("git-branchless");
        let branchless_path = cargo_bin_path
            .parent()
            .expect("Unable to find git-branchless path parent");
        let bash = get_sh().expect("bash missing?");
        let bash_path = bash.parent().unwrap();
        std::env::join_paths(vec![
            // For Git to be able to launch `git-branchless`.
            branchless_path.as_os_str(),
            // For our hooks to be able to call back into `git`.
            self.git_exec_path.as_os_str(),
            // For branchless to manually invoke bash when needed.
            bash_path.as_os_str(),
        ])
        .expect("joining paths")
    }

    /// Get the environment variables needed to run git in the test environment.
    pub fn get_base_env(&self, time: isize) -> Vec<(OsString, OsString)> {
        // Required for determinism, as these values will be baked into the commit
        // hash.
        let date: OsString = format!("{DUMMY_DATE} -{time:0>2}").into();

        // Fake "editor" which accepts the default contents of any commit
        // messages. Usually, we can set this with `git commit -m`, but we have
        // no such option for things such as `git rebase`, which may call `git
        // commit` later as a part of their execution.
        //
        // ":" is understood by `git` to skip editing.
        let git_editor = OsString::from(":");

        let new_path = self.get_path_for_env();
        let envs = vec![
            ("GIT_CONFIG_NOSYSTEM", OsString::from("1")),
            ("GIT_AUTHOR_DATE", date.clone()),
            ("GIT_COMMITTER_DATE", date),
            ("GIT_EDITOR", git_editor),
            ("GIT_EXEC_PATH", self.git_exec_path.as_os_str().into()),
            ("PATH", new_path),
            (TEST_GIT, self.path_to_git.as_os_str().into()),
            (
                TEST_SEPARATE_COMMAND_BINARIES,
                std::env::var_os(TEST_SEPARATE_COMMAND_BINARIES).unwrap_or_default(),
            ),
        ];

        envs.into_iter()
            .map(|(key, value)| (OsString::from(key), value))
            .collect()
    }

    #[track_caller]
    #[instrument]
    fn run_with_options_inner(
        &self,
        args: &[&str],
        options: &GitRunOptions,
    ) -> eyre::Result<(String, String)> {
        let GitRunOptions {
            time,
            expected_exit_code,
            input,
            env,
        } = options;

        let env: BTreeMap<_, _> = self
            .get_base_env(*time)
            .into_iter()
            .chain(
                env.iter()
                    .map(|(k, v)| (OsString::from(k), OsString::from(v))),
            )
            .collect();
        let mut command = Command::new(&self.path_to_git);
        command
            .current_dir(&self.repo_path)
            .args(args)
            .env_clear()
            .envs(&env);

        let result = if let Some(input) = input {
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            write!(child.stdin.take().unwrap(), "{}", &input)?;
            child.wait_with_output().wrap_err_with(|| {
                format!(
                    "Running git
                    Executable: {:?}
                    Args: {:?}
                    Stdin: {:?}
                    Env: <not shown>",
                    &self.path_to_git, &args, input
                )
            })?
        } else {
            command.output().wrap_err_with(|| {
                format!(
                    "Running git
                    Executable: {:?}
                    Args: {:?}
                    Env: <not shown>",
                    &self.path_to_git, &args
                )
            })?
        };

        let exit_code = result
            .status
            .code()
            .expect("Failed to read exit code from Git process");
        let result = if exit_code != *expected_exit_code {
            eyre::bail!(
                "Git command {:?} {:?} exited with unexpected code {} (expected {})
env:
{:#?}
stdout:
{}
stderr:
{}",
                &self.path_to_git,
                &args,
                exit_code,
                expected_exit_code,
                &env,
                &String::from_utf8_lossy(&result.stdout),
                &String::from_utf8_lossy(&result.stderr),
            )
        } else {
            result
        };
        let stdout = String::from_utf8(result.stdout)?;
        let stdout = self.preprocess_output(stdout)?;
        let stderr = String::from_utf8(result.stderr)?;
        let stderr = self.preprocess_output(stderr)?;
        Ok((stdout, stderr))
    }

    /// Run a Git command.
    #[track_caller]
    pub fn run_with_options<S: AsRef<str> + std::fmt::Debug>(
        &self,
        args: &[S],
        options: &GitRunOptions,
    ) -> eyre::Result<(String, String)> {
        self.run_with_options_inner(
            args.iter().map(|arg| arg.as_ref()).collect_vec().as_slice(),
            options,
        )
    }

    /// Run a Git command.
    #[track_caller]
    pub fn run<S: AsRef<str> + std::fmt::Debug>(
        &self,
        args: &[S],
    ) -> eyre::Result<(String, String)> {
        if let Some(first_arg) = args.first() {
            if first_arg.as_ref() == "branchless" {
                eyre::bail!(
                    r#"Refusing to invoke `branchless` via `git.run(&["branchless", ...])`; instead, call `git.branchless(&[...])`"#
                );
            }
        }

        self.run_with_options(args, &Default::default())
    }

    /// Render the smartlog for the repository.
    #[instrument]
    pub fn smartlog(&self) -> eyre::Result<String> {
        let (stdout, _stderr) = self.branchless("smartlog", &[])?;
        Ok(stdout)
    }

    /// Convenience method to call `branchless_with_options` with the default
    /// options.
    #[track_caller]
    #[instrument]
    pub fn branchless(&self, subcommand: &str, args: &[&str]) -> eyre::Result<(String, String)> {
        self.branchless_with_options(subcommand, args, &Default::default())
    }

    /// Locate the git-branchless binary and run a git-branchless subcommand
    /// with the provided `GitRunOptions`. These subcommands are located using
    /// `should_use_separate_command_binary`.
    #[track_caller]
    #[instrument]
    pub fn branchless_with_options(
        &self,
        subcommand: &str,
        args: &[&str],
        options: &GitRunOptions,
    ) -> eyre::Result<(String, String)> {
        let mut git_run_args = Vec::new();
        if should_use_separate_command_binary(subcommand) {
            git_run_args.push(format!("branchless-{subcommand}"));
        } else {
            git_run_args.push("branchless".to_string());
            git_run_args.push(subcommand.to_string());
        }
        git_run_args.extend(args.iter().map(|arg| arg.to_string()));

        let result = self.run_with_options(&git_run_args, options);

        if !should_use_separate_command_binary(subcommand) {
            let main_command_exe = assert_cmd::cargo::cargo_bin("git-branchless");
            let subcommand_exe =
                assert_cmd::cargo::cargo_bin(format!("git-branchless-{subcommand}"));
            if main_command_exe.exists() && subcommand_exe.exists() {
                let main_command_mtime = main_command_exe.metadata()?.modified()?;
                let subcommand_mtime = subcommand_exe.metadata()?.modified()?;
                if subcommand_mtime > main_command_mtime {
                    result.suggestion(format!(
                        "\
The modified time for {main_command_exe:?} was before the modified time for
{subcommand_exe:?}, which may indicate that you made changes to the subcommand
without building the main executable. This may cause spurious test failures
because the main executable code is out of date.

If so, you should either explicitly run: cargo -p git-branchless
to build the main executable before running this test; or, if it's okay to skip
building the main executable and test only the subcommand executable, you
can set the environment variable
`{TEST_SEPARATE_COMMAND_BINARIES}={subcommand}` to directly invoke it.\
"
                    ))
                } else {
                    result
                }
            } else {
                result
            }
        } else {
            result.suggestion(format!(
                "\
If you have set the {TEST_SEPARATE_COMMAND_BINARIES} environment variable, then \
the git-branchless-{subcommand} binary is NOT automatically built or updated when \
running integration tests for other binaries (see \
https://github.com/rust-lang/cargo/issues/4316 for more details).

Make sure that git-branchless-{subcommand} has been built before running \
integration tests. You can build it with: cargo build -p git-branchless-{subcommand}

If you have not set the {TEST_SEPARATE_COMMAND_BINARIES} environment variable, \
then you can only run tests in the main `git-branchless` and \
`git-branchless-lib` crates.\
        ",
            ))
        }
    }

    /// Set up a Git repo in the directory and initialize git-branchless to work
    /// with it.
    #[instrument]
    pub fn init_repo_with_options(&self, options: &GitInitOptions) -> eyre::Result<()> {
        self.run(&["init"])?;
        self.run(&["config", "user.name", DUMMY_NAME])?;
        self.run(&["config", "user.email", DUMMY_EMAIL])?;
        self.run(&["config", "core.abbrev", "7"])?;

        if options.make_initial_commit {
            self.commit_file("initial", 0)?;
        }

        // Non-deterministic metadata (depends on current time).
        self.run(&[
            "config",
            "branchless.commitDescriptors.relativeTime",
            "false",
        ])?;
        self.run(&["config", "branchless.restack.preserveTimestamps", "true"])?;

        // Disable warnings of the following form on Windows:
        //
        // ```
        // warning: LF will be replaced by CRLF in initial.txt.
        // The file will have its original line endings in your working directory
        // ```
        self.run(&["config", "core.autocrlf", "false"])?;

        if options.run_branchless_init {
            self.branchless("init", &[])?;
        }

        Ok(())
    }

    /// Set up a Git repo in the directory and initialize git-branchless to work
    /// with it.
    pub fn init_repo(&self) -> eyre::Result<()> {
        self.init_repo_with_options(&Default::default())
    }

    /// Clone this repository into the `target` repository (which must not have
    /// been initialized).
    pub fn clone_repo_into(&self, target: &Git, additional_args: &[&str]) -> eyre::Result<()> {
        let remote = format!("file://{}", self.repo_path.to_str().unwrap());
        let args = {
            let mut args = vec![
                "clone",
                // For Windows in CI.
                "-c",
                "core.autocrlf=false",
                &remote,
                target.repo_path.to_str().unwrap(),
            ];
            args.extend(additional_args.iter());
            args
        };

        let (_stdout, _stderr) = self.run(args.as_slice())?;

        // Configuration options are not inherited from the original repo, so
        // set them in the cloned repo.
        let new_repo = Git {
            repo_path: target.repo_path.clone(),
            ..self.clone()
        };
        new_repo.init_repo_with_options(&GitInitOptions {
            make_initial_commit: false,
            run_branchless_init: false,
        })?;

        Ok(())
    }

    /// Write the provided contents to the provided file in the repository root.
    /// For historical reasons, the name is suffixed with `.txt` (this is
    /// technical debt).
    pub fn write_file_txt(&self, name: &str, contents: &str) -> eyre::Result<()> {
        let name = format!("{name}.txt");
        self.write_file(&name, contents)
    }

    /// Write the provided contents to the provided file in the repository root.
    pub fn write_file(&self, name: &str, contents: &str) -> eyre::Result<()> {
        let path = self.repo_path.join(name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(self.repo_path.join(dir))?;
        }
        std::fs::write(&path, contents)?;
        Ok(())
    }

    /// Delete the provided file in the repository root.
    pub fn delete_file(&self, name: &str) -> eyre::Result<()> {
        let file_path = self.repo_path.join(format!("{name}.txt"));
        fs::remove_file(file_path)?;
        Ok(())
    }

    /// Delete the provided file in the repository root.
    pub fn set_file_permissions(
        &self,
        name: &str,
        permissions: fs::Permissions,
    ) -> eyre::Result<()> {
        let file_path = self.repo_path.join(format!("{name}.txt"));
        fs::set_permissions(file_path, permissions)?;
        Ok(())
    }

    /// Get a diff of a commit with the a/b file header removed. Only works for commits
    /// with a single file.
    #[instrument]
    pub fn get_trimmed_diff(&self, file: &str, commit: &str) -> eyre::Result<String> {
        let (stdout, _stderr) = self.run(&["show", "--pretty=format:", commit])?;
        let split_on = format!("+++ b/{file}\n");
        match stdout.as_str().split_once(split_on.as_str()) {
            Some((_, diff)) => Ok(diff.to_string()),
            None => eyre::bail!("Error trimming diff. Could not split on '{split_on}'"),
        }
    }

    /// Commit a file with given contents and message. The `time` argument is
    /// used to set the commit timestamp, which is factored into the commit
    /// hash. The filename is always appended to the message prefix.
    #[track_caller]
    #[instrument]
    pub fn commit_file_with_contents_and_message(
        &self,
        name: &str,
        time: isize,
        contents: &str,
        message_prefix: &str,
    ) -> eyre::Result<NonZeroOid> {
        let message = format!("{message_prefix} {name}.txt");
        self.write_file_txt(name, contents)?;
        self.run(&["add", "."])?;
        self.run_with_options(
            &["commit", "-m", &message],
            &GitRunOptions {
                time,
                ..Default::default()
            },
        )?;

        let repo = self.get_repo()?;
        let oid = repo
            .get_head_info()?
            .oid
            .expect("Could not find OID for just-created commit");
        Ok(oid)
    }

    /// Commit a file with given contents and a default message. The `time`
    /// argument is used to set the commit timestamp, which is factored into the
    /// commit hash.
    #[track_caller]
    #[instrument]
    pub fn commit_file_with_contents(
        &self,
        name: &str,
        time: isize,
        contents: &str,
    ) -> eyre::Result<NonZeroOid> {
        self.commit_file_with_contents_and_message(name, time, contents, "create")
    }

    /// Commit a file with default contents. The `time` argument is used to set
    /// the commit timestamp, which is factored into the commit hash.
    #[track_caller]
    #[instrument]
    pub fn commit_file(&self, name: &str, time: isize) -> eyre::Result<NonZeroOid> {
        self.commit_file_with_contents(name, time, &format!("{name} contents\n"))
    }

    /// Detach HEAD. This is useful to call to make sure that no branch is
    /// checked out, and therefore that future commit operations don't move any
    /// branches.
    #[instrument]
    pub fn detach_head(&self) -> eyre::Result<()> {
        self.run(&["checkout", "--detach"])?;
        Ok(())
    }

    /// Get a `Repo` object for this repository.
    #[instrument]
    pub fn get_repo(&self) -> eyre::Result<Repo> {
        let repo = Repo::from_dir(&self.repo_path)?;
        Ok(repo)
    }

    /// Get the version of the Git executable.
    #[instrument]
    pub fn get_version(&self) -> eyre::Result<GitVersion> {
        let (version_str, _stderr) = self.run(&["version"])?;
        let version = version_str.parse()?;
        Ok(version)
    }

    /// Get the `GitRunInfo` to use for this repository.
    #[instrument]
    pub fn get_git_run_info(&self) -> GitRunInfo {
        GitRunInfo {
            path_to_git: self.path_to_git.clone(),
            working_directory: self.repo_path.clone(),
            env: self.get_base_env(0).into_iter().collect(),
        }
    }

    /// Determine if the Git executable supports the `reference-transaction`
    /// hook.
    #[instrument]
    pub fn supports_reference_transactions(&self) -> eyre::Result<bool> {
        let version = self.get_version()?;
        Ok(version >= GitVersion(2, 29, 0))
    }

    /// Determine if the `--committer-date-is-author-date` option to `git rebase
    /// -i` is respected.
    ///
    /// This affects whether we can rely on the timestamps being preserved
    /// during a rebase when `branchless.restack.preserveTimestamps` is set.
    pub fn supports_committer_date_is_author_date(&self) -> eyre::Result<bool> {
        // The `--committer-date-is-author-date` option was previously passed
        // only to the `am` rebase back-end, until Git v2.29, when it became
        // available for merge back-end rebases as well.
        //
        // See https://git-scm.com/docs/git-rebase/2.28.0
        //
        // > These flags are passed to git am to easily change the dates of the
        // > rebased commits (see git-am[1]).
        // >
        // > See also INCOMPATIBLE OPTIONS below.
        //
        // See https://git-scm.com/docs/git-rebase/2.29.0
        //
        // > Instead of using the current time as the committer date, use the
        // > author date of the commit being rebased as the committer date. This
        // > option implies --force-rebase.
        let version = self.get_version()?;
        Ok(version >= GitVersion(2, 29, 0))
    }

    /// The `log.excludeDecoration` configuration option was introduced in Git
    /// v2.27.
    pub fn supports_log_exclude_decoration(&self) -> eyre::Result<bool> {
        let version = self.get_version()?;
        Ok(version >= GitVersion(2, 27, 0))
    }

    /// Git v2.44 produces `AUTO_MERGE` refs as part of some operations, which
    /// changes the event log according to the `reference-transaction` hook.
    pub fn produces_auto_merge_refs(&self) -> eyre::Result<bool> {
        let version = self.get_version()?;
        Ok(version >= GitVersion(2, 44, 0))
    }

    /// Resolve a file during a merge or rebase conflict with the provided
    /// contents.
    #[instrument]
    pub fn resolve_file(&self, name: &str, contents: &str) -> eyre::Result<()> {
        let file_path = self.repo_path.join(format!("{name}.txt"));
        std::fs::write(&file_path, contents)?;
        let file_path = match file_path.to_str() {
            None => eyre::bail!("Could not convert file path to string: {:?}", file_path),
            Some(file_path) => file_path,
        };
        self.run(&["add", file_path])?;
        Ok(())
    }

    /// Clear the event log on disk. Currently-existing commits will not have
    /// been observed by the new event log (once it's created by another
    /// command).
    #[instrument]
    pub fn clear_event_log(&self) -> eyre::Result<()> {
        let event_log_path = self.repo_path.join(".git/branchless/db.sqlite3");
        std::fs::remove_file(event_log_path)?;
        Ok(())
    }
}

/// Wrapper around a `Git` instance which cleans up the repository once dropped.
pub struct GitWrapper {
    repo_dir: TempDir,
    git: Git,
}

impl Deref for GitWrapper {
    type Target = Git;

    fn deref(&self) -> &Self::Target {
        &self.git
    }
}

/// From https://stackoverflow.com/a/65192210
/// License: CC-BY-SA 4.0
fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

impl GitWrapper {
    /// Make a copy of the repo on disk. This can be used to reuse testing
    /// setup.  This is *not* the same as running `git clone`; it's used to save
    /// initialization time as part of testing optimization.
    ///
    /// The copied repo will be deleted once the returned value has been dropped.
    pub fn duplicate_repo(&self) -> eyre::Result<Self> {
        let repo_dir = tempfile::tempdir()?;
        copy_dir_all(&self.repo_dir, &repo_dir)?;
        let git = Git {
            repo_path: repo_dir.path().to_path_buf(),
            ..self.git.clone()
        };
        Ok(Self { repo_dir, git })
    }
}

static COLOR_EYRE_INSTALL: OnceCell<()> = OnceCell::new();

/// Create a temporary directory for testing and a `Git` instance to use with it.
pub fn make_git() -> eyre::Result<GitWrapper> {
    COLOR_EYRE_INSTALL.get_or_try_init(color_eyre::install)?;

    let repo_dir = tempfile::tempdir()?;
    let path_to_git = get_path_to_git()?;
    let git_exec_path = get_git_exec_path()?;
    let git = Git::new(path_to_git, repo_dir.path().to_path_buf(), git_exec_path);
    Ok(GitWrapper { repo_dir, git })
}

/// Represents a pair of directories that will be cleaned up after this value
/// dropped. The two directories need to be `init`ed and `clone`ed by the
/// caller, respectively.
pub struct GitWrapperWithRemoteRepo {
    /// Guard to clean up the containing temporary directory. Make sure to bind
    /// this to a local variable not named `_`.
    pub temp_dir: TempDir,

    /// The wrapper around the original repository.
    pub original_repo: Git,

    /// The wrapper around the cloned repository.
    pub cloned_repo: Git,
}

/// Create a [`GitWrapperWithRemoteRepo`].
pub fn make_git_with_remote_repo() -> eyre::Result<GitWrapperWithRemoteRepo> {
    let path_to_git = get_path_to_git()?;
    let git_exec_path = get_git_exec_path()?;
    let temp_dir = tempfile::tempdir()?;
    let original_repo_path = temp_dir.path().join("original");
    std::fs::create_dir_all(&original_repo_path)?;
    let original_repo = Git::new(
        path_to_git.clone(),
        original_repo_path,
        git_exec_path.clone(),
    );
    let cloned_repo_path = temp_dir.path().join("cloned");
    let cloned_repo = Git::new(path_to_git, cloned_repo_path, git_exec_path);

    Ok(GitWrapperWithRemoteRepo {
        temp_dir,
        original_repo,
        cloned_repo,
    })
}

/// Represents a Git worktree for an existing Git repository on disk.
pub struct GitWorktreeWrapper {
    /// Guard to clean up the containing temporary directory. Make sure to bind
    /// this to a local variable not named `_`.
    pub temp_dir: TempDir,

    /// A wrapper around the worktree.
    pub worktree: Git,
}

/// Create a new worktree for the provided repository.
pub fn make_git_worktree(git: &Git, worktree_name: &str) -> eyre::Result<GitWorktreeWrapper> {
    let temp_dir = tempfile::tempdir()?;
    let worktree_path = temp_dir.path().join(worktree_name);
    git.run(&[
        "worktree",
        "add",
        "--detach",
        worktree_path.to_str().unwrap(),
    ])?;
    let worktree = Git {
        repo_path: worktree_path,
        ..git.clone()
    };
    Ok(GitWorktreeWrapper { temp_dir, worktree })
}

/// Find and extract the command to disable the hint mentioned in the output.
/// Returns the arguments to `git` which would disable the hint.
pub fn extract_hint_command(stdout: &str) -> Vec<String> {
    let hint_command = stdout
        .split_once("disable this hint by running: ")
        .map(|(_first, second)| second)
        .unwrap()
        .split('\n')
        .next()
        .unwrap();
    hint_command
        .split(' ')
        .skip(1) // "git"
        .filter(|s| s != &"--global")
        .map(|s| s.to_owned())
        .collect_vec()
}

/// Remove some of the output from `git rebase`, as it seems to be
/// non-deterministic as to whether or not it appears.
pub fn remove_rebase_lines(output: String) -> String {
    output
        .lines()
        .filter(|line| !line.contains("First, rewinding head") && !line.contains("Applying:"))
        .filter(|line| {
            // See https://github.com/arxanas/git-branchless/issues/87.  Before
            // Git v2.33 (`next` branch), the "Auto-merging" line appears
            // *after* the "CONFLICT" line for a given file (which doesn't make
            // sense -- how can there be a conflict before merging has started)?
            // The development version of Git v2.33 fixes this and places the
            // "Auto-merging" line *before* the "CONFLICT" line. To avoid having
            // to deal with multiple possible output formats, just remove the
            // line in question.
            !line.contains("Auto-merging")
            // Message changed between Git versions (due to be included in Git
            // v2.42) in
            // https://github.com/git/git/commit/d92304ff5cfdca463e9ecd1345807d0b46d6af33.
            && !line.contains("use \"git pull\"")
        })
        .flat_map(|line| [line, "\n"])
        .collect()
}

/// Remove whitespace from the end of each line in the provided string.
pub fn trim_lines(output: String) -> String {
    output
        .lines()
        .flat_map(|line| vec![line.trim_end(), "\n"].into_iter())
        .collect()
}

/// Remove lines which are not present or different between Git versions.
pub fn remove_nondeterministic_lines(output: String) -> String {
    output
        .lines()
        .filter(|line| {
            // This line is not present in some Git versions.
            !line.contains("Fetching")
                // This line is produced in a different order in some Git versions.
                && !line.contains("Your branch is up to date")
                // This line is only sometimes produced in CI for some reason? I
                // don't understand how it would only sometimes print this
                // message, but it does.
                && !line.contains("Switched to branch")
                // Hint text is more likely to change between Git versions.
                && !line.contains("hint:")
                // There are weird non-deterministic failures for test
                // `test_sync_no_delete_main_branch` where an extra newline is
                // printed, such as in
                // https://github.com/arxanas/git-branchless/actions/runs/5609690113/jobs/10263760651?pr=1002
                && !line.is_empty()
        })
        .flat_map(|line| [line, "\n"])
        .collect()
}

/// Utilities for testing in a virtual terminal (PTY).
pub mod pty {
    use std::sync::{mpsc::channel, Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use eyre::eyre;
    use portable_pty::{native_pty_system, CommandBuilder, ExitStatus, PtySize};

    use super::Git;

    /// Terminal escape code corresponding to pressing the up arrow key.
    pub const UP_ARROW: &str = "\x1b[A";

    /// Terminal escape code corresponding to pressing the down arrow key.
    pub const DOWN_ARROW: &str = "\x1b[B";

    /// An action to take as part of the PTY test script.
    pub enum PtyAction<'a> {
        /// Input the provided string as keystrokes to the terminal.
        Write(&'a str),

        /// Wait until the terminal display shows the provided string anywhere
        /// on the screen.
        WaitUntilContains(&'a str),
    }

    /// Run the provided script in the context of a virtual terminal.
    #[track_caller]
    pub fn run_in_pty(
        git: &Git,
        branchless_subcommand: &str,
        args: &[&str],
        inputs: &[PtyAction],
    ) -> eyre::Result<ExitStatus> {
        // Use the native pty implementation for the system
        let pty_system = native_pty_system();
        let pty_size = PtySize::default();
        let pty = pty_system
            .openpty(pty_size)
            .map_err(|e| eyre!("Could not open pty: {}", e))?;
        let mut pty_master = pty
            .master
            .take_writer()
            .map_err(|e| eyre!("Could not take PTY master writer: {e}"))?;

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
                                assert!(n < BUF_SIZE, "filled up PTY buffer by reading {n} bytes",);

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
                    if let Ok(Some(exit_status)) = child.try_wait() {
                        panic!(
                            "\
Tried to write {value:?} to PTY, but the process has already exited with status {exit_status:?}. Screen contents:
-----
{}
-----
", parser.lock().unwrap().screen().contents(),
                        );
                    }
                    write!(pty_master, "{value}")?;
                    pty_master.flush()?;
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
        let exit_status = child.wait()?;

        let _ = read_remainder_of_pty_output_thread;
        // Useful for debugging, but seems to deadlock on some tests:
        // let remainder_of_pty_output = read_remainder_of_pty_output_thread.join().unwrap();
        // assert!(
        //     !remainder_of_pty_output.contains("panic"),
        //     "Panic in PTY thread:\n{}",
        //     console::strip_ansi_codes(&remainder_of_pty_output)
        // );

        Ok(exit_status)
    }
}
