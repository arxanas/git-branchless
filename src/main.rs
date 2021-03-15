use std::convert::TryInto;
use std::io::{stderr, stdin, stdout};
use std::path::PathBuf;

use anyhow::Context;
use branchless::util::GitExecutable;
use simple_logger::SimpleLogger;
use structopt::StructOpt;

/// Branchless workflow for Git.
///
/// See the documentation at https://github.com/arxanas/git-branchless/wiki.
#[derive(StructOpt)]
#[structopt(version = "0.1.0", author = "Waleed Khan <me@waleedkhan.name>")]
enum Opts {
    /// Initialize the branchless workflow for this repository.
    Init,

    /// Display a nice graph of the commits you've recently worked on.
    Smartlog,

    /// Hide the provided commits from the smartlog.
    Hide {
        /// Zero or more commits to hide.
        ///
        /// Can either be hashes, like `abc123`, or ref-specs, like `HEAD^`.
        commits: Vec<String>,

        /// Also recursively hide all children commits of the provided commits.
        #[structopt(short = "-r", long = "--recursive")]
        recursive: bool,
    },

    /// Unhide previously-hidden commits from the smartlog.
    Unhide {
        /// Zero or more commits to unhide.
        ///
        /// Can either be hashes, like `abc123`, or ref-specs, like `HEAD^`.
        commits: Vec<String>,

        /// Also recursively unhide all children commits of the provided commits.
        #[structopt(short = "-r", long = "--recursive")]
        recursive: bool,
    },

    /// Move to an earlier commit in the current stack.
    Prev {
        /// The number of commits backward to go.
        num_commits: Option<isize>,
    },

    /// Move to a later commit in the current stack.
    Next {
        /// The number of commits forward to go.
        ///
        /// If not provided, defaults to 1.
        num_commits: Option<isize>,

        /// When encountering multiple next commits, choose the oldest.
        #[structopt(short = "-o", long = "--oldest")]
        oldest: bool,

        /// When encountering multiple next commits, choose the newest.
        #[structopt(short = "n", long = "--newest", conflicts_with("oldest"))]
        newest: bool,
    },

    /// Fix up commits abandoned by a previous rewrite operation.
    Restack,

    /// Browse or return to a previous state of the repository.
    Undo,

    /// Run internal garbage collection.
    Gc,

    /// Internal use.
    HookPreAutoGc,

    /// Internal use.
    HookPostRewrite { rewrite_type: String },

    /// Internal use.
    HookPostCheckout {
        previous_commit: String,
        current_commit: String,
        is_branch_checkout: isize,
    },

    /// Internal use.
    HookPostCommit,

    /// Internal use.
    HookReferenceTransaction { transaction_state: String },
}

fn main() -> anyhow::Result<()> {
    SimpleLogger::new()
        .init()
        .with_context(|| "Initializing logging")?;

    let opts = Opts::from_args();
    let mut stdin = stdin();
    let mut stdout = stdout();
    let mut stderr = stderr();
    let git_executable = std::env::var("PATH_TO_GIT").unwrap_or_else(|_| "git".to_string());
    let git_executable = GitExecutable(PathBuf::from(git_executable));

    let exit_code = match opts {
        Opts::Init => {
            branchless::init::init(&mut stdout, &git_executable)?;
            0
        }

        Opts::Smartlog => {
            branchless::smartlog::smartlog(&mut stdout)?;
            0
        }

        Opts::Hide { commits, recursive } => {
            branchless::hide::hide(&mut stdout, commits, recursive)?
        }

        Opts::Unhide { commits, recursive } => {
            branchless::hide::unhide(&mut stdout, commits, recursive)?
        }

        Opts::Prev { num_commits } => {
            branchless::navigation::prev(&mut stdout, &mut stderr, &&git_executable, num_commits)?
        }

        Opts::Next {
            num_commits,
            oldest,
            newest,
        } => {
            let towards = match (oldest, newest) {
                (false, false) => None,
                (true, false) => Some(branchless::navigation::Towards::Oldest),
                (false, true) => Some(branchless::navigation::Towards::Newest),
                (true, true) => anyhow::bail!("Both --oldest and --newest were set"),
            };
            branchless::navigation::next(
                &mut stdout,
                &mut stderr,
                &git_executable,
                num_commits,
                towards,
            )?
        }

        Opts::Restack => branchless::restack::restack(&mut stdout, &mut stderr, &git_executable)?,

        Opts::Undo => {
            branchless::undo::undo(&mut stdin, &mut stdout, &mut stderr, &git_executable)?
        }

        Opts::Gc | Opts::HookPreAutoGc => {
            branchless::gc::gc(&mut stdout)?;
            0
        }

        Opts::HookPostRewrite { rewrite_type } => {
            branchless::hooks::hook_post_rewrite(&mut stdout, &rewrite_type)?;
            0
        }

        Opts::HookPostCheckout {
            previous_commit,
            current_commit,
            is_branch_checkout,
        } => {
            branchless::hooks::hook_post_checkout(
                &mut stdout,
                &previous_commit,
                &current_commit,
                is_branch_checkout,
            )?;
            0
        }

        Opts::HookPostCommit => {
            branchless::hooks::hook_post_commit(&mut stdout)?;
            0
        }

        Opts::HookReferenceTransaction { transaction_state } => {
            branchless::hooks::hook_reference_transaction(&mut stdout, &transaction_state)?;
            0
        }
    };

    let exit_code: i32 = exit_code.try_into()?;
    std::process::exit(exit_code)
}
