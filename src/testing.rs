//! Testing utilities.
//!
//! This is inside `src` rather than `tests` since we use this code in some unit
//! tests.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::str::FromStr;

use crate::util::{get_sh, wrap_git_error, GitExecutable, GitVersion};
use anyhow::Context;
use fn_error_context::context;

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
    pub git_executable: PathBuf,
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
#[derive(Debug)]
pub struct GitRunOptions {
    /// The timestamp of the command. Mostly useful for `git commit`. This should
    /// be a number like 0, 1, 2, 3...
    pub time: isize,

    /// The exit code that `Git` should return.
    pub expected_exit_code: i32,

    /// The input to write to the child process's stdin.
    pub input: Option<String>,
}

impl Default for GitRunOptions {
    fn default() -> Self {
        GitRunOptions {
            time: 0,
            expected_exit_code: 0,
            input: None,
        }
    }
}

impl Git {
    /// Constructor.
    pub fn new(repo_path: PathBuf, git_executable: GitExecutable) -> Self {
        let GitExecutable(git_executable) = git_executable;
        Git {
            repo_path,
            git_executable,
        }
    }

    /// Replace dynamic strings in the output, for testing purposes.
    pub fn preprocess_stdout(&self, stdout: String) -> anyhow::Result<String> {
        let git_executable = self
            .git_executable
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Could not convert Git executable to string"))?;
        let stdout = stdout.replace(git_executable, "<git-executable>");
        Ok(stdout)
    }

    /// Get the `PATH` environment variable to use for testing.
    pub fn get_path_for_env(&self) -> String {
        let cargo_bin_path = assert_cmd::cargo::cargo_bin("git-branchless");
        let branchless_path = cargo_bin_path
            .parent()
            .expect("Unable to find git-branchless path parent");
        let git_path = self
            .git_executable
            .parent()
            .expect("Unable to find git path parent");
        let bash = get_sh().expect("bash missing?");
        let bash_path = bash.parent().unwrap();
        std::env::join_paths(
            vec![
                // For Git to be able to launch `git-branchless`.
                branchless_path,
                // For our hooks to be able to call back into `git`.
                git_path,
                // For branchless to manually invoke bash when needed.
                bash_path,
            ]
            .iter()
            .map(|path| path.to_str().expect("Unable to decode path component")),
        )
        .expect("joining paths")
        .to_string_lossy()
        .to_string()
    }

    /// Run a Git command.
    #[context("Running Git command with args: {:?} and options: {:?}", args, options)]
    pub fn run_with_options<S: AsRef<str> + std::fmt::Debug>(
        &self,
        args: &[S],
        options: &GitRunOptions,
    ) -> anyhow::Result<(String, String)> {
        let GitRunOptions {
            time,
            expected_exit_code,
            input,
        } = options;

        // Required for determinism, as these values will be baked into the commit
        // hash.
        let date = format!("{date} -{time:0>2}", date = DUMMY_DATE, time = time);

        let args: Vec<&str> = {
            let repo_path = self.repo_path.to_str().expect("Could not decode repo path");
            let mut new_args: Vec<&str> = vec!["-C", repo_path];
            new_args.extend(args.iter().map(|arg| arg.as_ref()));
            new_args
        };

        let new_path = self.get_path_for_env();
        let env: Vec<(&str, &str)> = vec![
            ("GIT_AUTHOR_DATE", &date),
            ("GIT_COMMITTER_DATE", &date),
            // Fake "editor" which accepts the default contents of any commit
            // messages. Usually, we can set this with `git commit -m`, but we have
            // no such option for things such as `git rebase`, which may call `git
            // commit` later as a part of their execution.
            //
            // ":" is understood by `git` to skip editing.
            ("GIT_EDITOR", ":"),
            (
                "PATH_TO_GIT",
                &self
                    .git_executable
                    .to_str()
                    .expect("Could not decode `git_executable`"),
            ),
            ("PATH", &new_path),
        ];

        let mut command = Command::new(&self.git_executable);
        command.args(&args).env_clear().envs(env.iter().copied());

        let result = if let Some(input) = input {
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            write!(child.stdin.take().unwrap(), "{}", &input)?;
            child.wait_with_output().with_context(|| {
                format!(
                    "Running git
                    Executable: {:?}
                    Args: {:?}
                    Stdin: {:?}
                    Env: <not shown>",
                    &self.git_executable, &args, input
                )
            })?
        } else {
            command.output().with_context(|| {
                format!(
                    "Running git
                    Executable: {:?}
                    Args: {:?}
                    Env: <not shown>",
                    &self.git_executable, &args
                )
            })?
        };

        let exit_code = result
            .status
            .code()
            .expect("Failed to read exit code from Git process");
        let result = if exit_code != *expected_exit_code {
            anyhow::bail!(
                "Git command {:?} {:?} exited with unexpected code {} (expected {})
                stdout:
                {}
                stderr:
                {}",
                &self.git_executable,
                &args,
                exit_code,
                expected_exit_code,
                &String::from_utf8_lossy(&result.stdout),
                &String::from_utf8_lossy(&result.stderr),
            )
        } else {
            result
        };
        let stdout = String::from_utf8(result.stdout)?;
        let stdout = self.preprocess_stdout(stdout)?;
        let stderr = String::from_utf8(result.stderr)?;
        Ok((stdout, stderr))
    }

    /// Run a Git command.
    pub fn run<S: AsRef<str> + std::fmt::Debug>(
        &self,
        args: &[S],
    ) -> anyhow::Result<(String, String)> {
        self.run_with_options(args, &Default::default())
    }

    /// Set up a Git repo in the directory and initialize git-branchless to work
    /// with it.
    #[context("Initializing Git repo with options: {:?}", options)]
    pub fn init_repo_with_options(&self, options: &GitInitOptions) -> anyhow::Result<()> {
        self.run(&["init"])?;
        self.run(&["config", "user.name", DUMMY_NAME])?;
        self.run(&["config", "user.email", DUMMY_EMAIL])?;

        if options.make_initial_commit {
            self.commit_file("initial", 0)?;
        }

        // Non-deterministic metadata (depends on current time).
        self.run(&["config", "branchless.commitMetadata.relativeTime", "false"])?;

        if options.run_branchless_init {
            self.run(&["branchless", "init"])?;
        }

        Ok(())
    }

    /// Set up a Git repo in the directory and initialize git-branchless to work
    /// with it.
    pub fn init_repo(&self) -> anyhow::Result<()> {
        self.init_repo_with_options(&Default::default())
    }

    /// Write the provided contents to the provided file in the repository root.
    pub fn write_file(&self, name: &str, contents: &str) -> anyhow::Result<()> {
        let file_path = self.repo_path.join(format!("{}.txt", name));
        std::fs::write(&file_path, contents)?;
        Ok(())
    }

    /// Commit a file with default contents. The `time` argument is used to set
    /// the commit timestamp, which is factored into the commit hash.
    #[context(
        "Committing file {:?} at time {:?} with contents: {:?}",
        name,
        time,
        contents
    )]
    pub fn commit_file_with_contents(
        &self,
        name: &str,
        time: isize,
        contents: &str,
    ) -> anyhow::Result<git2::Oid> {
        self.write_file(name, contents)?;
        self.run(&["add", "."])?;
        self.run_with_options(
            &["commit", "-m", &format!("create {}.txt", name)],
            &GitRunOptions {
                time,
                ..Default::default()
            },
        )?;

        let repo = self.get_repo()?;
        let oid = repo.head()?.peel_to_commit()?.id();
        Ok(oid)
    }

    /// Commit a file with default contents. The `time` argument is used to set
    /// the commit timestamp, which is factored into the commit hash.
    pub fn commit_file(&self, name: &str, time: isize) -> anyhow::Result<git2::Oid> {
        self.commit_file_with_contents(name, time, &format!("{} contents\n", name))
    }

    /// Detach HEAD. This is useful to call to make sure that no branch is
    /// checked out, and therefore that future commit operations don't move any
    /// branches.
    #[context("Detaching HEAD")]
    pub fn detach_head(&self) -> anyhow::Result<()> {
        self.run(&["checkout", "--detach"])?;
        Ok(())
    }

    /// Get a `git2::Repository` object for this repository.
    #[context("Getting the `git2::Repository` object for {:?}", self)]
    pub fn get_repo(&self) -> anyhow::Result<git2::Repository> {
        git2::Repository::open(&self.repo_path).map_err(wrap_git_error)
    }

    /// Get the version of the Git executable.
    #[context("Getting the Git version for {:?}", self)]
    pub fn get_version(&self) -> anyhow::Result<GitVersion> {
        let (version_str, _stderr) = self.run(&["version"])?;
        version_str.parse()
    }

    /// Determine if the Git executable supports the `reference-transaction`
    /// hook.
    #[context("Detecting reference-transaction support for {:?}", self)]
    pub fn supports_reference_transactions(&self) -> anyhow::Result<bool> {
        let version = self.get_version()?;
        Ok(version >= GitVersion(2, 29, 0))
    }

    /// Resolve a file during a merge or rebase conflict with the provided
    /// contents.
    #[context("Resolving file {:?} with contents: {:?}", name, contents)]
    pub fn resolve_file(&self, name: &str, contents: &str) -> anyhow::Result<()> {
        let file_path = self.repo_path.join(format!("{}.txt", name));
        std::fs::write(&file_path, contents)?;
        let file_path = match file_path.to_str() {
            None => anyhow::bail!("Could not convert file path to string: {:?}", file_path),
            Some(file_path) => file_path,
        };
        self.run(&["add", file_path])?;
        Ok(())
    }
}

/// Get the path to the Git executable for testing.
#[context("Getting the Git executable to use")]
pub fn get_git_executable() -> anyhow::Result<PathBuf> {
    let git_executable = std::env::var("PATH_TO_GIT").with_context(|| {
        "No path to git set. Try running as: PATH_TO_GIT=$(which git) cargo test ..."
    })?;
    let git_executable = PathBuf::from_str(&git_executable)?;
    Ok(git_executable)
}

/// Create a temporary directory for testing and a `Git` instance to use with it.
pub fn with_git(f: fn(Git) -> anyhow::Result<()>) -> anyhow::Result<()> {
    let repo_dir = tempfile::tempdir()?;
    let git_executable = get_git_executable()?;
    let git_executable = GitExecutable(git_executable);
    let git = Git::new(repo_dir.path().to_path_buf(), git_executable);
    f(git)
}
