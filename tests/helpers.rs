use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use branchless::util::{wrap_git_error, GitVersion};

const DUMMY_NAME: &str = "Testy McTestface";
const DUMMY_EMAIL: &str = "test@example.com";
const DUMMY_DATE: &str = "Wed 29 Oct 12:34:56 2020 PDT";

pub struct Git {
    pub repo_path: PathBuf,
    pub git_executable: PathBuf,
}

pub struct GitInitOptions {
    pub make_initial_commit: bool,
}

impl Default for GitInitOptions {
    fn default() -> Self {
        GitInitOptions {
            make_initial_commit: true,
        }
    }
}

pub struct GitRunOptions {
    pub time: isize,
    pub check: bool,
}

impl Default for GitRunOptions {
    fn default() -> Self {
        GitRunOptions {
            time: 0,
            check: true,
        }
    }
}

impl Git {
    fn new(repo_path: &Path, git_executable: &Path) -> Self {
        let repo_path = repo_path.to_path_buf();
        let git_executable = git_executable.to_path_buf();
        Git {
            repo_path,
            git_executable,
        }
    }

    fn build_command<S: AsRef<str>>(&self, args: &[S], options: &GitRunOptions) -> Command {
        // Required for determinism, as these values will be baked into the commit
        // hash.
        let date = format!("{date} -{time:0>2}", date = DUMMY_DATE, time = options.time);

        let args: Vec<&str> = {
            let repo_path = self.repo_path.to_str().expect("Could not decode repo path");
            let mut new_args: Vec<&str> = vec!["-C", repo_path];
            new_args.extend(args.iter().map(|arg| arg.as_ref()));
            new_args
        };

        let git_path = self
            .git_executable
            .parent()
            .expect("Unable to find git path parent");
        let cargo_bin_path = assert_cmd::cargo::cargo_bin("git-branchless");
        let branchless_path = cargo_bin_path
            .parent()
            .expect("Unable to find git-branchless path parent");
        let new_path = vec![git_path, branchless_path]
            .iter()
            .map(|path| path.to_str().expect("Unable to decode path component"))
            .collect::<Vec<_>>()
            .join(":");

        let env: Vec<(&str, &str)> = vec![
            ("GIT_AUTHOR_DATE", &date),
            ("GIT_COMMITTER_DATE", &date),
            // Fake "editor" which accepts the default contents of any commit
            // messages. Usually, we can set this with `git commit -m`, but we have
            // no such option for things such as `git rebase`, which may call `git
            // commit` later as a part of their execution.
            ("GIT_EDITOR", "true"),
            (
                "PATH_TO_GIT",
                self.git_executable
                    .to_str()
                    .expect("Could not decode `git_executable`"),
            ),
            ("PATH", &new_path),
        ];
        let mut command = Command::new(&self.git_executable);
        command.args(&args).env_clear().envs(env.iter().copied());
        command
    }

    fn preprocess_stdout(&self, stdout: String) -> anyhow::Result<String> {
        let git_executable = self
            .git_executable
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Could not convert Git executable to string"))?;
        let stdout = stdout.replace(git_executable, "<git-executable>");
        Ok(stdout)
    }

    pub fn run_with_options<S: AsRef<str> + std::fmt::Debug>(
        &self,
        args: &[S],
        options: &GitRunOptions,
    ) -> anyhow::Result<(String, String)> {
        let mut command = self.build_command(args, options);
        let result = command.output().with_context(|| {
            format!(
                "Running git
                Executable: {:?}
                Args: {:?}
                Env: <not shown>",
                &self.git_executable, &args
            )
        })?;

        let result = if options.check && !result.status.success() {
            anyhow::bail!(
                "Git command {:?} {:?} failed
                stdout:
                {}
                stderr:
                {}",
                &self.git_executable,
                &args,
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

    pub fn run<S: AsRef<str> + std::fmt::Debug>(
        &self,
        args: &[S],
    ) -> anyhow::Result<(String, String)> {
        self.run_with_options(args, &Default::default())
    }

    pub fn init_repo_with_options(&self, options: &GitInitOptions) -> anyhow::Result<()> {
        self.run(&["init"])?;
        self.run(&["config", "user.name", DUMMY_NAME])?;
        self.run(&["config", "user.email", DUMMY_EMAIL])?;

        if options.make_initial_commit {
            self.commit_file("initial", 0)?;
        }

        let mut python_source_root = assert_cmd::cargo::cargo_bin("git-branchless");
        python_source_root.pop();
        python_source_root.pop();
        let python_source_root = python_source_root.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "Could not convert Python source root path {:?} to string",
                &python_source_root,
            )
        })?;
        self.run(&[
            "config",
            "alias.branchless",
            &format!(
                "!env PYTHONPATH={} python -m branchless",
                python_source_root
            ),
        ])?;

        // Silence some log-spam.
        self.run(&["config", "advice.detachedHead", "false"])?;

        // Non-deterministic metadata (depends on current time).
        self.run(&["config", "branchless.commitMetadata.relativeTime", "false"])?;

        self.run(&["branchless", "init"])?;

        Ok(())
    }

    pub fn init_repo(&self) -> anyhow::Result<()> {
        self.init_repo_with_options(&Default::default())
    }

    pub fn commit_file_with_contents(
        &self,
        name: &str,
        time: isize,
        contents: &str,
    ) -> anyhow::Result<()> {
        let file_path = self.repo_path.join(format!("{}.txt", name));
        let mut file = File::create(file_path)?;
        file.write_all(contents.as_bytes())?;
        self.run(&["add", "."])?;
        self.run_with_options(
            &["commit", "-m", &format!("create {}.txt", name)],
            &GitRunOptions {
                time,
                ..Default::default()
            },
        )?;
        Ok(())
    }

    pub fn commit_file(&self, name: &str, time: isize) -> anyhow::Result<()> {
        self.commit_file_with_contents(name, time, &format!("{} contents\n", name))
    }

    pub fn detach_head(&self) -> anyhow::Result<()> {
        self.run(&["checkout", "--detach"])?;
        Ok(())
    }

    pub fn get_repo(&self) -> anyhow::Result<git2::Repository> {
        git2::Repository::open(&self.repo_path).map_err(wrap_git_error)
    }

    pub fn get_version(&self) -> anyhow::Result<GitVersion> {
        let (version_str, _stderr) = self.run(&["version"])?;
        version_str.parse()
    }
}

pub fn with_git(f: fn(Git) -> anyhow::Result<()>) -> anyhow::Result<()> {
    let repo_dir = tempfile::tempdir()?;
    let git_executable = std::env::var("PATH_TO_GIT")
        .expect("No path to git set. Try running as: PATH_TO_GIT=$(which git) cargo test ...");
    let git = Git::new(Path::new(repo_dir.path()), Path::new(&git_executable));
    f(git)
}
