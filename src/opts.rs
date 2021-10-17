//! The command-line options for `git-branchless`.

use clap::{App, Clap, IntoApp};
use man::{Arg, Author, Manual, Opt};
use std::path::{Path, PathBuf};

/// A command wrapped by `git-branchless wrap`. The arguments are forwarded to
/// `git`.
#[derive(Clap)]
pub enum WrappedCommand {
    /// The wrapped command.
    #[clap(external_subcommand)]
    WrappedCommand(Vec<String>),
}

/// FIXME: write man-page text
#[derive(Clap)]
pub enum Command {
    /// Initialize the branchless workflow for this repository.
    Init {
        /// Uninstall the branchless workflow instead of initializing it.
        #[clap(long = "uninstall")]
        uninstall: bool,
    },

    /// Display a nice graph of the commits you've recently worked on.
    Smartlog {
        /// Also show commits which have been hidden.
        #[clap(long = "hidden")]
        show_hidden_commits: bool,
    },

    /// Hide the provided commits from the smartlog.
    Hide {
        /// Zero or more commits to hide.
        ///
        /// Can either be hashes, like `abc123`, or ref-specs, like `HEAD^`.
        commits: Vec<String>,

        /// Also recursively hide all visible children commits of the provided
        /// commits.
        #[clap(short = 'r', long = "recursive")]
        recursive: bool,
    },

    /// Unhide previously-hidden commits from the smartlog.
    Unhide {
        /// Zero or more commits to unhide.
        ///
        /// Can either be hashes, like `abc123`, or ref-specs, like `HEAD^`.
        commits: Vec<String>,

        /// Also recursively unhide all children commits of the provided commits.
        #[clap(short = 'r', long = "recursive")]
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
        #[clap(short = 'o', long = "oldest")]
        oldest: bool,

        /// When encountering multiple next commits, choose the newest.
        #[clap(short = 'n', long = "newest", conflicts_with("oldest"))]
        newest: bool,
    },

    /// Move a subtree of commits from one location to another.
    ///
    /// By default, `git move` tries to move the entire current stack if you
    /// don't pass a `--source` or `--base` option (equivalent to writing
    /// `--base HEAD`).
    ///
    /// By default, `git move` attempts to rebase all commits in-memory. If you
    /// want to force an on-disk rebase, pass the `--on-disk` flag. Note that
    /// `post-commit` hooks are not called during in-memory rebases.
    Move {
        /// The source commit to move. This commit, and all of its descendants,
        /// will be moved.
        #[clap(short = 's', long = "source")]
        source: Option<String>,

        /// A commit inside a subtree to move. The entire subtree, starting from
        /// the main branch, will be moved, not just the commits descending from
        /// this commit.
        #[clap(short = 'b', long = "base", conflicts_with = "source")]
        base: Option<String>,

        /// The destination commit to move all source commits onto. If not
        /// provided, defaults to the current commit.
        #[clap(short = 'd', long = "dest")]
        dest: Option<String>,

        /// Only attempt to perform an in-memory rebase. If it fails, do not
        /// attempt an on-disk rebase.
        #[clap(long = "in-memory", conflicts_with = "force-on-disk")]
        force_in_memory: bool,

        /// Skip attempting to use an in-memory rebase, and try an
        /// on-disk rebase directly.
        #[clap(long = "on-disk")]
        force_on_disk: bool,

        /// Debugging option. Print the constraints used to create the rebase
        /// plan before executing it.
        #[clap(long = "debug-dump-rebase-constraints")]
        dump_rebase_constraints: bool,

        /// Debugging option. Print the rebase plan that will be executed before
        /// executing it.
        #[clap(long = "debug-dump-rebase-plan")]
        dump_rebase_plan: bool,
    },

    /// Fix up commits abandoned by a previous rewrite operation.
    Restack {
        /// The IDs of the abandoned commits whose descendants should be
        /// restacked. If not provided, all abandoned commits are restacked.
        commits: Vec<String>,

        /// Only attempt to perform an in-memory rebase. If it fails, do not
        /// attempt an on-disk rebase.
        #[clap(long = "in-memory", conflicts_with = "force-on-disk")]
        force_in_memory: bool,

        /// Skip attempting to use an in-memory rebase, and try an
        /// on-disk rebase directly.
        #[clap(long = "on-disk")]
        force_on_disk: bool,

        /// Debugging option. Print the constraints used to create the rebase
        /// plan before executing it.
        #[clap(long = "debug-dump-rebase-constraints")]
        dump_rebase_constraints: bool,

        /// Debugging option. Print the rebase plan that will be executed before
        /// executing it.
        #[clap(long = "debug-dump-rebase-plan")]
        dump_rebase_plan: bool,
    },

    /// Browse or return to a previous state of the repository.
    Undo,

    /// Run internal garbage collection.
    Gc,

    /// Wrap a Git command inside a branchless transaction.
    Wrap {
        /// The `git` executable to invoke.
        #[clap(long = "git-executable")]
        git_executable: Option<PathBuf>,

        /// The arguments to pass to `git`.
        #[clap(subcommand)]
        command: WrappedCommand,
    },

    /// Internal use.
    HookPreAutoGc,

    /// Internal use.
    HookPostRewrite {
        /// One of `amend` or `rebase`.
        rewrite_type: String,
    },

    /// Internal use.
    HookRegisterExtraPostRewriteHook,

    /// Internal use.
    HookDetectEmptyCommit {
        /// The OID of the commit currently being applied, to be checked for emptiness.
        old_commit_oid: String,
    },

    /// Internal use.
    HookSkipUpstreamAppliedCommit {
        /// The OID of the commit that was skipped.
        commit_oid: String,
    },

    /// Internal use.
    HookPostCheckout {
        /// The previous commit OID.
        previous_commit: String,

        /// The current commit OID.
        current_commit: String,

        /// Whether or not this was a branch checkout (versus a file checkout).
        is_branch_checkout: isize,
    },

    /// Internal use.
    HookPostCommit,

    /// Internal use.
    HookPostMerge {
        /// Whether or not this is a squash merge. See githooks(5).
        is_squash_merge: isize,
    },

    /// Internal use.
    HookReferenceTransaction {
        /// One of `prepared`, `committed`, or `aborted`. See githooks(5).
        transaction_state: String,
    },
}

/// Branchless workflow for Git.
///
/// See the documentation at https://github.com/arxanas/git-branchless/wiki.
#[derive(Clap)]
#[clap(version = env!("CARGO_PKG_VERSION"), author = "Waleed Khan <me@waleedkhan.name>")]
pub struct Opts {
    /// Change to the given directory before executing the rest of the program.
    /// (The option is called `-C` for symmetry with Git.)
    #[clap(short = 'C')]
    pub working_directory: Option<PathBuf>,

    /// The `git-branchless` subcommand to run.
    #[clap(subcommand)]
    pub command: Command,
}

/// Generate and write man-pages into the specified directory.
///
/// The generated files are named things like `man1/git-branchless-smartlog.1`,
/// so this directory should be of the form `path/to/man`, to ensure that these
/// files get generated into the correct man-page section.
pub fn write_man_pages(man_dir: &Path) -> std::io::Result<()> {
    let man1_dir = man_dir.join("man1");
    std::fs::create_dir_all(&man1_dir)?;

    let app = Opts::into_app();
    generate_man_page(&man1_dir, "git-branchless", &app)?;
    for subcommand in app.get_subcommands() {
        let subcommand_exe_name = format!("git-branchless-{}", subcommand.get_name());
        generate_man_page(&man1_dir, &subcommand_exe_name, subcommand)?;
    }
    Ok(())
}

fn generate_man_page(man1_dir: &Path, name: &str, command: &App) -> std::io::Result<()> {
    let mut manual = Manual::new(name);
    if let Some(about) = command.get_about() {
        manual = manual.about(about);
    }

    let authors = env!("CARGO_PKG_AUTHORS").split(':').map(|author_string| {
        let (name, email) = match author_string.split_once(" <") {
            Some(value) => value,
            None => panic!(
                "Invalid author specifier (should be Full Name <email@example.com>): {:?}",
                author_string
            ),
        };

        let email = email.strip_prefix('<').unwrap_or(email);
        let email = email.strip_suffix('>').unwrap_or(email);
        Author::new(name).email(email)
    });
    for author in authors {
        manual = manual.author(author);
    }

    {
        // `clap==3.0.0-beta.4` does not have a `get_long_about` method, which
        // seems to be an omission. See
        // https://github.com/clap-rs/clap/pull/2843.
        let mut buf = Vec::new();
        command
            .clone()
            .help_template("{about}")
            .write_long_help(&mut buf)?;
        let long_help = String::from_utf8(buf).expect("Argument help should be UTF-8");
        manual = manual.description(long_help);
    }

    for arg in command.get_positionals() {
        manual = manual.arg(Arg::new(&format!("[{}]", arg.get_name().to_uppercase())));
    }

    for flag in command.get_flags() {
        let opt = Opt::new(flag.get_name());
        let opt = match flag.get_short() {
            Some(short) => opt.short(&String::from(short)),
            None => opt,
        };
        let opt = match flag.get_long() {
            Some(long) => opt.long(long),
            None => opt,
        };
        let opt = match flag.get_about() {
            Some(help) => opt.help(help),
            None => opt,
        };
        manual = manual.option(opt);
    }

    // FIXME: implement rest of man-page rendering.

    let output_path = man1_dir.join(format!("{}.1", name));
    std::fs::write(output_path, manual.render())?;
    Ok(())
}
