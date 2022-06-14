//! The command-line options for `git-branchless`.

use clap::{ArgEnum, Args, Command as ClapCommand, IntoApp, Parser};
use lib::git::NonZeroOid;
use man::Arg;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// A revset expression. Can be a commit hash, branch name, or one of the
/// various revset functions.
#[derive(Clone, Debug)]
pub struct Revset(pub String);

impl FromStr for Revset {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

/// A command wrapped by `git-branchless wrap`. The arguments are forwarded to
/// `git`.
#[derive(Parser)]
pub enum WrappedCommand {
    /// The wrapped command.
    #[clap(external_subcommand)]
    WrappedCommand(Vec<String>),
}

/// Options for moving commits.
#[derive(Args, Debug)]
pub struct MoveOptions {
    /// Only attempt to perform an in-memory rebase. If it fails, do not
    /// attempt an on-disk rebase.
    #[clap(action, long = "in-memory", conflicts_with_all(&["force-on-disk", "merge"]))]
    pub force_in_memory: bool,

    /// Skip attempting to use an in-memory rebase, and try an
    /// on-disk rebase directly.
    #[clap(action, long = "on-disk")]
    pub force_on_disk: bool,

    /// Don't attempt to deduplicate commits. Normally, a commit with the same
    /// contents as another commit which has already been applied to the target
    /// branch is skipped. If set, this flag skips that check.
    #[clap(action(clap::ArgAction::SetFalse), long = "no-deduplicate-commits")]
    pub detect_duplicate_commits_via_patch_id: bool,

    /// Attempt to resolve merge conflicts, if any. If a merge conflict
    /// occurs and this option is not set, the operation is aborted.
    #[clap(action, name = "merge", short = 'm', long = "merge")]
    pub resolve_merge_conflicts: bool,

    /// Debugging option. Print the constraints used to create the rebase
    /// plan before executing it.
    #[clap(action, long = "debug-dump-rebase-constraints")]
    pub dump_rebase_constraints: bool,

    /// Debugging option. Print the rebase plan that will be executed before
    /// executing it.
    #[clap(action, long = "debug-dump-rebase-plan")]
    pub dump_rebase_plan: bool,
}

/// Options for traversing commits.
#[derive(Args, Debug)]
pub struct TraverseCommitsOptions {
    /// The number of commits to traverse.
    ///
    /// If not provided, defaults to 1.
    #[clap(value_parser)]
    pub num_commits: Option<usize>,

    /// Traverse as many commits as possible.
    #[clap(action, short = 'a', long = "all")]
    pub all_the_way: bool,

    /// Move the specified number of branches rather than commits.
    #[clap(action, short = 'b', long = "branch")]
    pub move_by_branches: bool,

    /// When encountering multiple next commits, choose the oldest.
    #[clap(action, short = 'o', long = "oldest")]
    pub oldest: bool,

    /// When encountering multiple next commits, choose the newest.
    #[clap(action, short = 'n', long = "newest", conflicts_with("oldest"))]
    pub newest: bool,

    /// When encountering multiple next commits, interactively prompt which to
    /// advance to.
    #[clap(
        action,
        short = 'i',
        long = "interactive",
        conflicts_with("newest"),
        conflicts_with("oldest")
    )]
    pub interactive: bool,

    /// If the local changes conflict with the destination commit, attempt to
    /// merge them.
    #[clap(action, short = 'm', long = "merge")]
    pub merge: bool,

    /// If the local changes conflict with the destination commit, discard them.
    /// (Use with caution!)
    #[clap(action, short = 'f', long = "force", conflicts_with("merge"))]
    pub force: bool,
}

/// Options for checking out a commit.
#[derive(Args, Debug)]
pub struct CheckoutOptions {
    /// Interactively select a commit to check out.
    #[clap(action, short = 'i', long = "interactive")]
    pub interactive: bool,

    /// When checking out the target commit, also create a branch with the
    /// provided name pointing to that commit.
    #[clap(value_parser, short = 'b', long = "branch")]
    pub branch_name: Option<String>,

    /// Forcibly switch commits, discarding any working copy changes if
    /// necessary.
    #[clap(action, short = 'f', long = "force")]
    pub force: bool,

    /// If the current working copy changes do not apply cleanly to the
    /// target commit, start merge conflict resolution instead of aborting.
    #[clap(action, short = 'm', long = "merge", conflicts_with("force"))]
    pub merge: bool,

    /// The commit or branch to check out.
    ///
    /// If this is not provided, then interactive commit selection starts as
    /// if `--interactive` were passed.
    ///
    /// If this is provided and the `--interactive` flag is passed, this
    /// text is used to pre-fill the interactive commit selector.
    #[clap(value_parser)]
    pub target: Option<String>,
}

/// FIXME: write man-page text
#[derive(Parser)]
pub enum Command {
    /// Amend the current HEAD commit.
    Amend {
        /// Options for moving commits.
        #[clap(flatten)]
        move_options: MoveOptions,
    },

    /// Gather information about recent operations to upload as part of a bug
    /// report.
    BugReport,

    /// Check out a given commit.
    Checkout {
        /// Options for checking out a commit.
        #[clap(flatten)]
        checkout_options: CheckoutOptions,
    },

    /// Run internal garbage collection.
    Gc,

    /// Hide the provided commits from the smartlog.
    Hide {
        /// Zero or more commits to hide.
        #[clap(value_parser)]
        revsets: Vec<Revset>,

        /// Also delete any branches that are abandoned as a result of this hide.
        #[clap(action, short = 'D', long = "delete-branches")]
        delete_branches: bool,

        /// Also recursively hide all visible children commits of the provided
        /// commits.
        #[clap(action, short = 'r', long = "recursive")]
        recursive: bool,
    },

    /// Internal use.
    #[clap(hide = true)]
    HookDetectEmptyCommit {
        /// The OID of the commit currently being applied, to be checked for emptiness.
        #[clap(value_parser)]
        old_commit_oid: String,
    },

    /// Internal use.
    #[clap(hide = true)]
    HookPreAutoGc,

    /// Internal use.
    #[clap(hide = true)]
    HookPostCheckout {
        /// The previous commit OID.
        #[clap(value_parser)]
        previous_commit: String,

        /// The current commit OID.
        #[clap(value_parser)]
        current_commit: String,

        /// Whether or not this was a branch checkout (versus a file checkout).
        #[clap(value_parser)]
        is_branch_checkout: isize,
    },

    /// Internal use.
    #[clap(hide = true)]
    HookPostCommit,

    /// Internal use.
    #[clap(hide = true)]
    HookPostMerge {
        /// Whether or not this is a squash merge. See githooks(5).
        #[clap(value_parser)]
        is_squash_merge: isize,
    },

    /// Internal use.
    #[clap(hide = true)]
    HookPostRewrite {
        /// One of `amend` or `rebase`.
        #[clap(value_parser)]
        rewrite_type: String,
    },

    /// Internal use.
    #[clap(hide = true)]
    HookReferenceTransaction {
        /// One of `prepared`, `committed`, or `aborted`. See githooks(5).
        #[clap(value_parser)]
        transaction_state: String,
    },

    /// Internal use.
    #[clap(hide = true)]
    HookRegisterExtraPostRewriteHook,

    /// Internal use.
    #[clap(hide = true)]
    HookSkipUpstreamAppliedCommit {
        /// The OID of the commit that was skipped.
        #[clap(value_parser)]
        commit_oid: String,
    },

    /// Initialize the branchless workflow for this repository.
    Init {
        /// Uninstall the branchless workflow instead of initializing it.
        #[clap(action, long = "uninstall")]
        uninstall: bool,

        /// Use the provided name as the name of the main branch.
        ///
        /// If not set, it will be auto-detected. If it can't be auto-detected,
        /// then you will be prompted to enter a value for the main branch name.
        #[clap(value_parser, long = "main-branch", conflicts_with = "uninstall")]
        main_branch_name: Option<String>,
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
        #[clap(value_parser, short = 's', long = "source")]
        source: Option<Revset>,

        /// A commit inside a subtree to move. The entire subtree, starting from
        /// the main branch, will be moved, not just the commits descending from
        /// this commit.
        #[clap(value_parser, short = 'b', long = "base", conflicts_with = "source")]
        base: Option<Revset>,

        /// The destination commit to move all source commits onto. If not
        /// provided, defaults to the current commit.
        #[clap(value_parser, short = 'd', long = "dest")]
        dest: Option<Revset>,

        /// Options for moving commits.
        #[clap(flatten)]
        move_options: MoveOptions,
    },

    /// Move to a later commit in the current stack.
    Next {
        /// Options for traversing commits.
        #[clap(flatten)]
        traverse_commits_options: TraverseCommitsOptions,
    },

    /// Move to an earlier commit in the current stack.
    Prev {
        /// Options for traversing commits.
        #[clap(flatten)]
        traverse_commits_options: TraverseCommitsOptions,
    },

    /// Query the commit graph using the "revset" DSL.
    Query {
        /// The query to execute.
        #[clap(value_parser)]
        revset: Revset,
    },

    /// Fix up commits abandoned by a previous rewrite operation.
    Restack {
        /// The IDs of the abandoned commits whose descendants should be
        /// restacked. If not provided, all abandoned commits are restacked.
        #[clap(value_parser)]
        commits: Vec<Revset>,

        /// Options for moving commits.
        #[clap(flatten)]
        move_options: MoveOptions,
    },

    /// Create a commit by interactively selecting which changes to include.
    Record,

    /// Reword commits.
    Reword {
        /// Zero or more commits to reword. If not provided, defaults to "HEAD".
        ///
        /// Can either be hashes, like `abc123`, or ref-specs, like `HEAD^`.
        #[clap(value_parser)]
        revsets: Vec<Revset>,

        /// Message to apply to commits. Multiple messages will be combined as separate paragraphs,
        /// similar to `git commit`.
        #[clap(value_parser, short = 'm', long = "message")]
        messages: Vec<String>,

        /// Throw away the original commit messages.
        ///
        /// If `commit.template` is set, then the editor is pre-populated with
        /// that; otherwise, the editor starts empty.
        #[clap(action, short = 'd', long = "discard", conflicts_with("messages"))]
        discard: bool,
    },

    /// Display a nice graph of the commits you've recently worked on.
    Smartlog {
        /// Also show commits which have been hidden.
        #[clap(action, long = "hidden")]
        show_hidden_commits: bool,

        /// Only show commits that exist on a branch.
        #[clap(action, long = "only-branches", conflicts_with = "show-hidden-commits")]
        only_show_branches: bool,
    },

    #[clap(hide = true)]
    /// Manage working copy snapshots.
    Snapshot {
        /// The subcommand to run.
        #[clap(subcommand)]
        subcommand: SnapshotSubcommand,
    },

    /// Move any local commit stacks on top of the main branch.
    Sync {
        /// Run `git fetch` to update remote references before carrying out the
        /// sync.
        #[clap(
            action,
            short = 'p',
            long = "pull",
            visible_short_alias = 'u',
            visible_alias = "--update"
        )]
        update_refs: bool,

        /// Force rebasing commits even if they're already based on top of their
        /// destination.
        #[clap(action, short = 'f', long = "force")]
        force: bool,

        /// Options for moving commits.
        #[clap(flatten)]
        move_options: MoveOptions,

        /// The commits whose stacks will be moved on top of the main branch. If
        /// no commits are provided, all draft commits will be synced.
        #[clap(value_parser)]
        revsets: Vec<Revset>,
    },

    /// Browse or return to a previous state of the repository.
    Undo {
        /// Interactively browse through previous states of the repository
        /// before selecting one to return to.
        #[clap(action, short = 'i', long = "interactive")]
        interactive: bool,

        /// Skip confirmation and apply changes immediately.
        #[clap(action, short = 'y', long = "yes")]
        yes: bool,
    },

    /// Unhide previously-hidden commits from the smartlog.
    Unhide {
        /// Zero or more commits to unhide.
        #[clap(value_parser)]
        revsets: Vec<Revset>,

        /// Also recursively unhide all children commits of the provided commits.
        #[clap(action, short = 'r', long = "recursive")]
        recursive: bool,
    },

    /// Wrap a Git command inside a branchless transaction.
    Wrap {
        /// The `git` executable to invoke.
        #[clap(value_parser, long = "git-executable")]
        git_executable: Option<PathBuf>,

        /// The arguments to pass to `git`.
        #[clap(subcommand)]
        command: WrappedCommand,
    },
}

/// Whether to display terminal colors.
#[derive(ArgEnum, Clone)]
pub enum ColorSetting {
    /// Automatically determine whether to display colors from the terminal and environment variables.
    /// This is the default behavior.
    Auto,
    /// Always display terminal colors.
    Always,
    /// Never display terminal colors.
    Never,
}

/// Branchless workflow for Git.
///
/// See the documentation at <https://github.com/arxanas/git-branchless/wiki>.
#[derive(Parser)]
#[clap(version = env!("CARGO_PKG_VERSION"), author = "Waleed Khan <me@waleedkhan.name>")]
pub struct Opts {
    /// Change to the given directory before executing the rest of the program.
    /// (The option is called `-C` for symmetry with Git.)
    #[clap(value_parser, short = 'C')]
    pub working_directory: Option<PathBuf>,

    /// Flag to force enable or disable terminal colors.
    #[clap(value_parser, long = "color", arg_enum)]
    pub color: Option<ColorSetting>,

    /// The `git-branchless` subcommand to run.
    #[clap(subcommand)]
    pub command: Command,
}

/// `snapshot` subcommands.
#[derive(Parser)]
pub enum SnapshotSubcommand {
    /// Create a new snapshot containing the working copy contents, and then
    /// reset the working copy to the current `HEAD` commit.
    ///
    /// On success, prints the snapshot commit hash to stdout.
    Create,

    /// Restore the working copy contents from the provided snapshot.
    Restore {
        /// The commit hash for the snapshot.
        #[clap(value_parser)]
        snapshot_oid: NonZeroOid,
    },
}

/// Generate and write man-pages into the specified directory.
///
/// The generated files are named things like `man1/git-branchless-smartlog.1`,
/// so this directory should be of the form `path/to/man`, to ensure that these
/// files get generated into the correct man-page section.
pub fn write_man_pages(man_dir: &Path) -> std::io::Result<()> {
    let man1_dir = man_dir.join("man1");
    std::fs::create_dir_all(&man1_dir)?;

    let app = Opts::command();
    generate_man_page(&man1_dir, "git-branchless", &app)?;
    for subcommand in app.get_subcommands() {
        let subcommand_exe_name = format!("git-branchless-{}", subcommand.get_name());
        generate_man_page(&man1_dir, &subcommand_exe_name, subcommand)?;
    }
    Ok(())
}

fn generate_man_page(man1_dir: &Path, name: &str, command: &ClapCommand) -> std::io::Result<()> {
    let mut manual = man::Manual::new(name);
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
        man::Author::new(name).email(email)
    });
    for author in authors {
        manual = manual.author(author);
    }

    if let Some(long_about) = command.get_long_about() {
        manual = manual.description(long_about);
    }

    for arg in command.get_positionals() {
        manual = manual.arg(Arg::new(&format!("[{}]", arg.get_id().to_uppercase())));
    }

    for flag in command.get_opts() {
        let opt = man::Opt::new(flag.get_id());
        let opt = match flag.get_short() {
            Some(short) => opt.short(&String::from(short)),
            None => opt,
        };
        let opt = match flag.get_long() {
            Some(long) => opt.long(long),
            None => opt,
        };
        let opt = match flag.get_help() {
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
