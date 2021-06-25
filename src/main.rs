use std::convert::TryInto;
use std::path::{Path, PathBuf};

use anyhow::Context;
use branchless::commands::wrap;
use branchless::util::GitExecutable;
use simple_logger::SimpleLogger;
use structopt::StructOpt;

#[derive(StructOpt)]
enum WrappedCommand {
    #[structopt(external_subcommand)]
    WrappedCommand(Vec<String>),
}

/// Branchless workflow for Git.
///
/// See the documentation at https://github.com/arxanas/git-branchless/wiki.
#[derive(StructOpt)]
#[structopt(version = env!("CARGO_PKG_VERSION"), author = "Waleed Khan <me@waleedkhan.name>")]
enum Opts {
    /// Initialize the branchless workflow for this repository.
    Init {
        /// Uninstall the branchless workflow instead of initializing it.
        #[structopt(long = "--uninstall")]
        uninstall: bool,
    },

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
        #[structopt(short = "-n", long = "--newest", conflicts_with("oldest"))]
        newest: bool,
    },

    /// Move a subtree of commits from one location to another.
    ///
    /// By default, `git move` attempts to rebase all commits in-memory. If you
    /// want to force an on-disk rebase, pass the `--on-disk` flag. Note that
    /// `post-commit` hooks are not called during in-memory rebases.
    Move {
        /// The source commit to move. This commit, and all of its descendants,
        /// will be moved. If not provided, defaults to the current commit.
        #[structopt(short = "-s", long = "--source")]
        source: Option<String>,

        /// A commit inside a subtree to move. The entire subtree, starting from
        /// the main branch, will be moved, not just the commits descending from
        /// this commit.
        #[structopt(short = "-b", long = "--base", conflicts_with = "source")]
        base: Option<String>,

        /// The destination commit to move all source commits onto. If not
        /// provided, defaults to the current commit.
        #[structopt(short = "-d", long = "--dest")]
        dest: Option<String>,

        /// Skip attempting to use an in-memory rebase, and try an
        /// on-disk rebase directly.
        #[structopt(long = "--on-disk")]
        force_on_disk: bool,
    },

    /// Fix up commits abandoned by a previous rewrite operation.
    Restack,

    /// Browse or return to a previous state of the repository.
    Undo,

    /// Run internal garbage collection.
    Gc,

    /// Wrap a Git command inside a branchless transaction.
    Wrap {
        #[structopt(long = "--git-executable")]
        git_executable: Option<PathBuf>,

        #[structopt(subcommand)]
        command: WrappedCommand,
    },

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
    let git_executable = std::env::var("PATH_TO_GIT").unwrap_or_else(|_| "git".to_string());
    let git_executable = Path::new(&git_executable);
    let git_executable = GitExecutable(git_executable.to_path_buf());

    let exit_code = match opts {
        Opts::Init { uninstall: false } => {
            branchless::commands::init::init(&git_executable)?;
            0
        }

        Opts::Init { uninstall: true } => {
            branchless::commands::init::uninstall()?;
            0
        }

        Opts::Smartlog => {
            branchless::commands::smartlog::smartlog()?;
            0
        }

        Opts::Hide { commits, recursive } => branchless::commands::hide::hide(commits, recursive)?,

        Opts::Unhide { commits, recursive } => {
            branchless::commands::hide::unhide(commits, recursive)?
        }

        Opts::Prev { num_commits } => {
            branchless::commands::navigation::prev(&&git_executable, num_commits)?
        }

        Opts::Next {
            num_commits,
            oldest,
            newest,
        } => {
            let towards = match (oldest, newest) {
                (false, false) => None,
                (true, false) => Some(branchless::commands::navigation::Towards::Oldest),
                (false, true) => Some(branchless::commands::navigation::Towards::Newest),
                (true, true) => anyhow::bail!("Both --oldest and --newest were set"),
            };
            branchless::commands::navigation::next(&git_executable, num_commits, towards)?
        }

        Opts::Move {
            source,
            dest,
            base,
            force_on_disk,
        } => branchless::commands::r#move::r#move(
            &git_executable,
            source,
            dest,
            base,
            force_on_disk,
        )?,

        Opts::Restack => branchless::commands::restack::restack(&git_executable)?,

        Opts::Undo => branchless::commands::undo::undo(&git_executable)?,

        Opts::Gc | Opts::HookPreAutoGc => {
            branchless::commands::gc::gc()?;
            0
        }

        Opts::Wrap {
            git_executable: explicit_git_executable,
            command: WrappedCommand::WrappedCommand(args),
        } => {
            let git_executable = match explicit_git_executable {
                Some(path) => GitExecutable(path),
                None => git_executable,
            };
            wrap::wrap(&git_executable, args.as_slice())?;
            0
        }

        Opts::HookPostRewrite { rewrite_type } => {
            branchless::commands::hooks::hook_post_rewrite(&rewrite_type)?;
            0
        }

        Opts::HookPostCheckout {
            previous_commit,
            current_commit,
            is_branch_checkout,
        } => {
            branchless::commands::hooks::hook_post_checkout(
                &previous_commit,
                &current_commit,
                is_branch_checkout,
            )?;
            0
        }

        Opts::HookPostCommit => {
            branchless::commands::hooks::hook_post_commit()?;
            0
        }

        Opts::HookReferenceTransaction { transaction_state } => {
            branchless::commands::hooks::hook_reference_transaction(&transaction_state)?;
            0
        }
    };

    let exit_code: i32 = exit_code.try_into()?;
    std::process::exit(exit_code)
}
