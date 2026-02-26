//! The command-line options for `git-branchless`.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_conditions)]
// These URLs are printed verbatim in help output, so we don't want to add extraneous Markdown
// formatting.
#![allow(rustdoc::bare_urls)]

use std::ffi::OsString;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use clap::{Args, Command as ClapCommand, CommandFactory, Parser, ValueEnum};
use lib::core::untracked_file_cache::UntrackedFileStrategy;
use lib::git::NonZeroOid;

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

impl Display for Revset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A command wrapped by `git-branchless wrap`. The arguments are forwarded to
/// `git`.
#[derive(Debug, Parser)]
pub enum WrappedCommand {
    /// The wrapped command.
    #[clap(external_subcommand)]
    WrappedCommand(Vec<String>),
}

/// Options for resolving revset expressions.
#[derive(Args, Debug, Default)]
pub struct ResolveRevsetOptions {
    /// Include hidden commits in the results of evaluating revset expressions.
    #[clap(action, long = "hidden")]
    pub show_hidden_commits: bool,
}

/// Options for moving commits.
#[derive(Args, Debug)]
pub struct MoveOptions {
    /// Force moving public commits, even though other people may have access to
    /// those commits.
    #[clap(action, short = 'f', long = "force-rewrite", visible_alias = "fr")]
    pub force_rewrite_public_commits: bool,

    /// Only attempt to perform an in-memory rebase. If it fails, do not
    /// attempt an on-disk rebase.
    #[clap(action, long = "in-memory", conflicts_with_all(&["force_on_disk", "merge"]))]
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
pub struct SwitchOptions {
    /// Interactively select a commit to check out.
    #[clap(action, short = 'i', long = "interactive")]
    pub interactive: bool,

    /// When checking out the target commit, also create a branch with the
    /// provided name pointing to that commit.
    #[clap(value_parser, short = 'c', long = "create")]
    pub branch_name: Option<String>,

    /// Forcibly switch commits, discarding any working copy changes if
    /// necessary.
    #[clap(action, short = 'f', long = "force")]
    pub force: bool,

    /// If the current working copy changes do not apply cleanly to the
    /// target commit, start merge conflict resolution instead of aborting.
    #[clap(action, short = 'm', long = "merge", conflicts_with("force"))]
    pub merge: bool,

    /// If the target is a branch, switch to that branch and immediately detach
    /// from it.
    #[clap(action, short = 'd', long = "detach")]
    pub detach: bool,

    /// The commit or branch to check out. If not provided, defaults to the
    /// current commit.
    ///
    /// If a revset is provided, it must evaluate to set with exactly 1 head.
    ///
    /// If this is provided and the `--interactive` flag is passed, this
    /// text is used to pre-fill the interactive commit selector.
    #[clap(value_parser)]
    pub target: Option<Revset>,
}

/// Internal use.
#[derive(Debug, Parser)]
pub enum HookSubcommand {
    /// Internal use.
    DetectEmptyCommit {
        /// The OID of the commit currently being applied, to be checked for emptiness.
        #[clap(value_parser)]
        old_commit_oid: String,
    },
    /// Internal use.
    PreAutoGc,
    /// Internal use.
    PostApplypatch,
    /// Internal use.
    PostCheckout {
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
    PostCommit,
    /// Internal use.
    PostMerge {
        /// Whether or not this is a squash merge. See githooks(5).
        #[clap(value_parser)]
        is_squash_merge: isize,
    },
    /// Internal use.
    PostRewrite {
        /// One of `amend` or `rebase`.
        #[clap(value_parser)]
        rewrite_type: String,
    },
    /// Internal use.
    ReferenceTransaction {
        /// One of `prepared`, `committed`, or `aborted`. See githooks(5).
        #[clap(value_parser)]
        transaction_state: String,
    },
    /// Internal use.
    RegisterExtraPostRewriteHook,
    /// Internal use.
    SkipUpstreamAppliedCommit {
        /// The OID of the commit that was skipped.
        #[clap(value_parser)]
        commit_oid: String,
    },
}

/// Internal use.
#[derive(Debug, Parser)]
pub struct HookArgs {
    /// The subcommand to run.
    #[clap(subcommand)]
    pub subcommand: HookSubcommand,
}

/// Initialize the branchless workflow for this repository.
#[derive(Debug, Parser)]
pub struct InitArgs {
    /// Uninstall the branchless workflow instead of initializing it.
    #[clap(action, long = "uninstall")]
    pub uninstall: bool,

    /// Use the provided name as the name of the main branch.
    ///
    /// If not set, it will be auto-detected. If it can't be auto-detected,
    /// then you will be prompted to enter a value for the main branch name.
    #[clap(value_parser, long = "main-branch", conflicts_with = "uninstall")]
    pub main_branch_name: Option<String>,
}

/// Install git-branchless's man-pages to the given path.
#[derive(Debug, Parser)]
pub struct InstallManPagesArgs {
    /// The path to install to. An example path might be `/usr/share/man`. The
    /// provded path will be appended with `man1`, etc., as appropriate.
    pub path: PathBuf,
}

/// Query the commit graph using the "revset" language and print matching
/// commits.
///
/// See https://github.com/arxanas/git-branchless/wiki/Reference:-Revsets to
/// learn more about revsets.
///
/// The outputted commits are guaranteed to be topologically sorted, with
/// ancestor commits appearing first.
#[derive(Debug, Parser)]
pub struct QueryArgs {
    /// The query to execute.
    #[clap(value_parser)]
    pub revset: Revset,

    /// Options for resolving revset expressions.
    #[clap(flatten)]
    pub resolve_revset_options: ResolveRevsetOptions,

    /// Print the branches attached to the resulting commits, rather than the commits themselves.
    #[clap(action, short = 'b', long = "branches")]
    pub show_branches: bool,

    /// Print the OID of each matching commit, one per line. This output is
    /// stable for use in scripts.
    #[clap(action, short = 'r', long = "raw", conflicts_with("show_branches"))]
    pub raw: bool,
}

/// Specify commit messages
#[derive(Debug, Parser)]
pub struct MessageArgs {
    /// The commit message to use. Multiple messages will be combined
    /// as separate paragraphs, similar to `git commit`.
    /// If not provided, you will be prompted to provide a commit message
    /// interactively.
    #[clap(value_parser, short = 'm', long = "message")]
    pub messages: Vec<String>,

    /// A commit to "fix up". The message will be prefixed with `fixup!`
    /// following the supplied commit, suitable for use with `git rebase --autosquash`.
    #[clap(value_parser, long = "fixup", conflicts_with_all(&["messages"]))]
    pub commit_to_fixup: Option<Revset>,
}

/// Create a commit by interactively selecting which changes to include.
#[derive(Debug, Parser)]
pub struct RecordArgs {
    /// Options for supplying commit messages.
    #[clap(flatten)]
    pub message_args: MessageArgs,

    /// Select changes to include interactively, rather than using the
    /// current staged/unstaged changes.
    #[clap(action, short = 'i', long = "interactive")]
    pub interactive: bool,

    /// Create and switch to a new branch with the given name before
    /// committing.
    #[clap(action, short = 'c', long = "create")]
    pub create: Option<String>,

    /// Detach the current branch before committing.
    #[clap(action, short = 'd', long = "detach", conflicts_with("create"))]
    pub detach: bool,

    /// Insert the new commit between the current commit and its children,
    /// if any.
    #[clap(action, short = 'I', long = "insert")]
    pub insert: bool,

    /// After making the new commit, switch back to the previous commit.
    #[clap(action, short = 's', long = "stash", conflicts_with_all(&["create", "detach"]))]
    pub stash: bool,

    /// How should newly encountered, untracked files be handled?
    #[clap(value_parser, long = "untracked", conflicts_with_all(&["interactive"]))]
    pub untracked_file_strategy: Option<UntrackedFileStrategy>,
}

/// Display a nice graph of the commits you've recently worked on.
#[derive(Debug, Parser)]
pub struct SmartlogArgs {
    /// The point in time at which to show the smartlog. If not provided,
    /// renders the smartlog as of the current time. If negative, is treated
    /// as an offset from the current event.
    #[clap(value_parser, long = "event-id")]
    pub event_id: Option<isize>,

    /// The commits to render. These commits, plus any related commits, will
    /// be rendered.
    #[clap(value_parser)]
    pub revset: Option<Revset>,

    /// Print the smartlog in the opposite of the usual order, with the latest
    /// commits first. (DEPRECATED: should be configured with `branchless.smartlog.reverse`)
    #[clap(long)]
    pub reverse: bool,

    /// Don't automatically add HEAD and the main branch to the list of commits
    /// to present. They will still be added if included in the revset.
    #[clap(long)]
    pub exact: bool,

    /// Options for resolving revset expressions.
    #[clap(flatten)]
    pub resolve_revset_options: ResolveRevsetOptions,
}

/// The Git hosting provider to use, called a "forge".
#[derive(Clone, Debug, ValueEnum)]
pub enum ForgeKind {
    /// Force-push branches to the default push remote. You can configure the
    /// default push remote with `git config remote.pushDefault <remote>`.
    Branch,

    /// Force-push branches to the remote and create a pull request for each
    /// branch using the `gh` command-line tool. WARNING: likely buggy!
    Github,

    /// Submit code reviews to Phabricator using the `arc` command-line tool.
    Phabricator,
}

/// Push commits to a remote.
#[derive(Debug, Parser)]
pub struct SubmitArgs {
    /// The commits to push to the forge. Unless `--create` is passed, this will
    /// only push commits that already have associated remote objects on the
    /// forge.
    #[clap(value_parser, default_value = "stack()")]
    pub revsets: Vec<Revset>,

    /// Options for resolving revset expressions.
    #[clap(flatten)]
    pub resolve_revset_options: ResolveRevsetOptions,

    /// The Git hosting provider to use, called a "forge". If not provided, an
    /// attempt will be made to automatically detect the forge used by the
    /// repository. If no forge can be detected, will fall back to the "branch"
    /// forge.
    #[clap(short = 'F', long = "forge")]
    pub forge_kind: Option<ForgeKind>,

    /// If there is no associated remote commit or code review object for a
    /// given local commit, create the remote object by pushing the local commit
    /// to the forge.
    #[clap(action, short = 'c', long = "create")]
    pub create: bool,

    /// If the forge supports it, create code reviews in "draft" mode.
    #[clap(action, short = 'd', long = "draft")]
    pub draft: bool,

    /// If the forge supports it, an optional message to include with the create
    /// or update operation.
    #[clap(short = 'm', long = "message")]
    pub message: Option<String>,

    /// If the forge supports it, how many jobs to execute in parallel. The
    /// value `0` indicates to use all CPUs.
    #[clap(short = 'j', long = "jobs")]
    pub num_jobs: Option<usize>,

    /// If the forge supports it and uses a tool that needs access to the
    /// working copy, what kind of execution strategy to use.
    #[clap(short = 's', long = "strategy")]
    pub execution_strategy: Option<TestExecutionStrategy>,

    /// Don't push or create anything. Instead, report what would be pushed or
    /// created. (This may still trigger fetching information from the forge.)
    #[clap(short = 'n', long = "dry-run")]
    pub dry_run: bool,
}

/// Run a command on each commit in a given set and aggregate the results.
#[derive(Debug, Parser)]
pub struct TestArgs {
    /// The subcommand to run.
    #[clap(subcommand)]
    pub subcommand: TestSubcommand,
}

/// FIXME: write man-page text
#[derive(Debug, Parser)]
pub enum Command {
    /// Amend the current HEAD commit.
    Amend {
        /// Options for moving commits.
        #[clap(flatten)]
        move_options: MoveOptions,

        /// Modify the contents of the current HEAD commit, but keep all contents of descendant
        /// commits exactly the same (i.e. "reparent" them). This can be useful when applying
        /// formatting or refactoring changes.
        #[clap(long)]
        reparent: bool,

        /// How should newly encountered, untracked files be handled?
        #[clap(action, long = "untracked")]
        untracked_file_strategy: Option<UntrackedFileStrategy>,
    },

    /// Gather information about recent operations to upload as part of a bug
    /// report.
    BugReport,

    /// Use the partial commit selector UI as a Git-compatible difftool; see
    /// git-difftool(1) for more information on Git difftools.
    Difftool(scm_diff_editor::Opts),

    /// Run internal garbage collection.
    Gc,

    /// Hide the provided commits from the smartlog.
    Hide {
        /// Zero or more commits to hide.
        #[clap(value_parser)]
        revsets: Vec<Revset>,

        /// Options for resolving revset expressions.
        #[clap(flatten)]
        resolve_revset_options: ResolveRevsetOptions,

        /// Don't delete branches that point to commits that would be hidden.
        /// (Those commits will remain visible as a result.)
        #[clap(action, long = "no-delete-branches")]
        no_delete_branches: bool,

        /// Also recursively hide all visible children commits of the provided
        /// commits.
        #[clap(action, short = 'r', long = "recursive")]
        recursive: bool,
    },

    /// Internal use.
    #[clap(hide = true)]
    Hook(HookArgs),

    /// Initialize the branchless workflow for this repository.
    Init(InitArgs),

    /// Install git-branchless's man-pages to the given path.
    InstallManPages(InstallManPagesArgs),

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
        #[clap(action(clap::ArgAction::Append), short = 's', long = "source")]
        source: Vec<Revset>,

        /// A commit inside a subtree to move. The entire subtree, starting from
        /// the main branch, will be moved, not just the commits descending from
        /// this commit.
        #[clap(
            action(clap::ArgAction::Append),
            short = 'b',
            long = "base",
            conflicts_with = "source"
        )]
        base: Vec<Revset>,

        /// A set of specific commits to move. These will be removed from their
        /// current locations and any unmoved children will be moved to their
        /// nearest unmoved ancestor.
        #[clap(
            action(clap::ArgAction::Append),
            short = 'x',
            long = "exact",
            conflicts_with_all(&["source", "base"])
        )]
        exact: Vec<Revset>,

        /// The destination commit to move all source commits onto. If not
        /// provided, defaults to the current commit.
        #[clap(value_parser, short = 'd', long = "dest")]
        dest: Option<Revset>,

        /// Options for resolving revset expressions.
        #[clap(flatten)]
        resolve_revset_options: ResolveRevsetOptions,

        /// Options for moving commits.
        #[clap(flatten)]
        move_options: MoveOptions,

        /// Combine the moved commits and squash them into the destination commit.
        #[clap(action, short = 'F', long = "fixup", conflicts_with = "insert")]
        fixup: bool,

        /// Insert the subtree between the destination and it's children, if any.
        /// Only supported if the moved subtree has a single head.
        #[clap(action, short = 'I', long = "insert")]
        insert: bool,

        /// Test whether an in-memory rebase would succeed.
        #[clap(action, long = "dry-run", conflicts_with = "force_on_disk")]
        dry_run: bool,
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

    /// Query the commit graph using the "revset" language and print matching
    /// commits.
    ///
    /// See https://github.com/arxanas/git-branchless/wiki/Reference:-Revsets to
    /// learn more about revsets.
    ///
    /// The outputted commits are guaranteed to be topologically sorted, with
    /// ancestor commits appearing first.
    Query(QueryArgs),

    /// Restore internal invariants by reconciling the internal operation log
    /// with the state of the Git repository.
    Repair {
        /// Apply changes.
        #[clap(action(clap::ArgAction::SetFalse), long = "no-dry-run")]
        dry_run: bool,
    },

    /// Fix up commits abandoned by a previous rewrite operation.
    Restack {
        /// The IDs of the abandoned commits whose descendants should be
        /// restacked. If not provided, all abandoned commits are restacked.
        #[clap(value_parser, default_value = "draft()")]
        revsets: Vec<Revset>,

        /// Options for resolving revset expressions.
        #[clap(flatten)]
        resolve_revset_options: ResolveRevsetOptions,

        /// Options for moving commits.
        #[clap(flatten)]
        move_options: MoveOptions,
    },

    /// Create a commit by interactively selecting which changes to include.
    Record(RecordArgs),

    /// Reword commits.
    Reword {
        /// Zero or more commits to reword.
        #[clap(
            value_parser,
            default_value = "stack() | @",
            default_value_if("commit_to_fixup", clap::builder::ArgPredicate::IsPresent, "@"),
            default_value_if("messages", clap::builder::ArgPredicate::IsPresent, "@")
        )]
        revsets: Vec<Revset>,

        /// Options for resolving revset expressions.
        #[clap(flatten)]
        resolve_revset_options: ResolveRevsetOptions,

        /// Force rewording public commits, even though other people may have access to
        /// those commits.
        #[clap(action, short = 'f', long = "force-rewrite", visible_alias = "fr")]
        force_rewrite_public_commits: bool,

        /// Options for supplying commit messages.
        #[clap(flatten)]
        message_args: MessageArgs,

        /// Throw away the original commit messages.
        ///
        /// If `commit.template` is set, then the editor is pre-populated with
        /// that; otherwise, the editor starts empty.
        #[clap(action, short = 'd', long = "discard", conflicts_with_all(&["messages", "commit_to_fixup"]))]
        discard: bool,
    },

    /// `smartlog` command.
    Smartlog(SmartlogArgs),

    #[clap(hide = true)]
    /// Manage working copy snapshots.
    Snapshot {
        /// The subcommand to run.
        #[clap(subcommand)]
        subcommand: SnapshotSubcommand,
    },

    /// Split commits.
    Split {
        /// Commit to split. If a revset is given, it must resolve to a single commit.
        #[clap(value_parser)]
        revset: Revset,

        /// Files to extract from the commit.
        #[clap(value_parser, required = true)]
        files: Vec<String>,

        /// Insert the extracted commit before (as a parent of) the split commit.
        #[clap(action, short = 'b', long)]
        before: bool,

        /// Restack any descendents onto the split commit, not the extracted commit.
        #[clap(action, short = 'd', long)]
        detach: bool,

        /// After extracting the changes, don't recommit them.
        #[clap(action, short = 'D', long = "discard", conflicts_with("detach"))]
        discard: bool,

        /// Options for resolving revset expressions.
        #[clap(flatten)]
        resolve_revset_options: ResolveRevsetOptions,

        /// Options for moving commits.
        #[clap(flatten)]
        move_options: MoveOptions,
    },

    /// Push commits to a remote.
    Submit(SubmitArgs),

    /// Switch to the provided branch or commit.
    Switch {
        /// Options for switching.
        #[clap(flatten)]
        switch_options: SwitchOptions,
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
        pull: bool,

        /// Options for moving commits.
        #[clap(flatten)]
        move_options: MoveOptions,

        /// The commits whose stacks will be moved on top of the main branch. If
        /// no commits are provided, all draft commits will be synced.
        #[clap(value_parser)]
        revsets: Vec<Revset>,

        /// Options for resolving revset expressions.
        #[clap(flatten)]
        resolve_revset_options: ResolveRevsetOptions,
    },

    /// Run a command on each commit in a given set and aggregate the results.
    Test(TestArgs),

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

        /// Options for resolving revset expressions.
        #[clap(flatten)]
        resolve_revset_options: ResolveRevsetOptions,

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
#[derive(Clone, Debug, ValueEnum)]
pub enum ColorSetting {
    /// Automatically determine whether to display colors from the terminal and environment variables.
    /// This is the default behavior.
    Auto,
    /// Always display terminal colors.
    Always,
    /// Never display terminal colors.
    Never,
}

/// How to execute tests.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum TestExecutionStrategy {
    /// Default. Run the tests in the working copy. This requires a clean working copy. This is
    /// useful if you want to reuse build artifacts in the current directory.
    WorkingCopy,

    /// Run the tests in a separate worktree (managed by git-branchless). This is useful if you want
    /// to run tests in parallel, or if you want to run tests on a different commit without
    /// invalidating build artifacts in the current directory, or if you want to run tests while
    /// your working copy is dirty.
    Worktree,
}

/// How to conduct searches on the commit graph.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum TestSearchStrategy {
    /// Visit commits starting from the earliest commit and exit early when a
    /// failing commit is found.
    Linear,

    /// Visit commits starting from the latest commit and exit early when a
    /// passing commit is found.
    Reverse,

    /// Visit commits starting from the middle of the commit graph and exit
    /// early when a failing commit is found.
    Binary,
}

/// Arguments which apply to all commands. Used during setup.
#[derive(Debug, Parser)]
pub struct GlobalArgs {
    /// Change to the given directory before executing the rest of the program.
    /// (The option is called `-C` for symmetry with Git.)
    #[clap(value_parser, short = 'C', global = true)]
    pub working_directory: Option<PathBuf>,

    /// Flag to force enable or disable terminal colors.
    #[clap(value_parser, long = "color", value_enum, global = true)]
    pub color: Option<ColorSetting>,
}

/// Branchless workflow for Git.
///
/// See the documentation at https://github.com/arxanas/git-branchless/wiki.
#[derive(Debug, Parser)]
#[clap(version = env!("CARGO_PKG_VERSION"), author = "Waleed Khan <me@waleedkhan.name>")]
pub struct Opts {
    /// Global arguments.
    #[clap(flatten)]
    pub global_args: GlobalArgs,

    /// The `git-branchless` subcommand to run.
    #[clap(subcommand)]
    pub command: Command,
}

/// `snapshot` subcommands.
#[derive(Debug, Parser)]
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

/// `test` subcommands.
#[derive(Debug, Parser)]
pub enum TestSubcommand {
    /// Clean any cached test results.
    Clean {
        /// The set of commits whose results should be cleaned.
        #[clap(value_parser, default_value = "stack() | @")]
        revset: Revset,

        /// Options for resolving revset expressions.
        #[clap(flatten)]
        resolve_revset_options: ResolveRevsetOptions,
    },

    /// Run a given command on a set of commits and present the successes and failures.
    Run {
        /// An ad-hoc command to execute on each commit.
        #[clap(value_parser, short = 'x', long = "exec")]
        exec: Option<String>,

        /// The test command alias for the command to execute on each commit. Set with
        /// `git config branchless.test.alias.<name> <command>`.
        #[clap(value_parser, short = 'c', long = "command", conflicts_with("exec"))]
        command: Option<String>,

        /// The set of commits to test.
        #[clap(value_parser, default_value = "stack() | @")]
        revset: Revset,

        /// Options for resolving revset expressions.
        #[clap(flatten)]
        resolve_revset_options: ResolveRevsetOptions,

        /// Show the test output as well.
        #[clap(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
        verbosity: u8,

        /// How to execute the tests.
        #[clap(short = 's', long = "strategy")]
        strategy: Option<TestExecutionStrategy>,

        /// Search for the first commit that fails the test command, rather than
        /// running on all commits.
        #[clap(short = 'S', long = "search")]
        search: Option<TestSearchStrategy>,

        /// Shorthand for `--search binary`.
        #[clap(short = 'b', long = "bisect", conflicts_with("search"))]
        bisect: bool,

        /// Don't read or write to the cache when executing the test commands.
        #[clap(long = "no-cache")]
        no_cache: bool,

        /// Run the test command in the foreground rather than the background so
        /// that the user can interact with it.
        #[clap(short = 'i', long = "interactive")]
        interactive: bool,

        /// How many jobs to execute in parallel. The value `0` indicates to use all CPUs.
        #[clap(short = 'j', long = "jobs")]
        jobs: Option<usize>,
    },

    /// Show the results of a set of previous test runs.
    Show {
        /// An ad-hoc command to execute on each commit.
        #[clap(value_parser, short = 'x', long = "exec")]
        exec: Option<String>,

        /// The test command alias for the command to execute on each commit. Set with
        /// `git config branchless.test.alias.<name> <command>`.
        #[clap(value_parser, short = 'c', long = "command", conflicts_with("exec"))]
        command: Option<String>,

        /// The set of commits to show the test output for.
        #[clap(value_parser, default_value = "stack() | @")]
        revset: Revset,

        /// Options for resolving revset expressions.
        #[clap(flatten)]
        resolve_revset_options: ResolveRevsetOptions,

        /// Show the test output as well.
        #[clap(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
        verbosity: u8,
    },

    /// Run a given command on a set of commits and present the successes and failures.
    Fix {
        /// An ad-hoc command to execute on each commit.
        #[clap(value_parser, short = 'x', long = "exec")]
        exec: Option<String>,

        /// The test command alias for the command to execute on each commit. Set with
        /// `git config branchless.test.alias.<name> <command>`.
        #[clap(value_parser, short = 'c', long = "command", conflicts_with("exec"))]
        command: Option<String>,

        /// Don't rewrite any commits. Instead, just print a summary as usual.
        #[clap(value_parser, short = 'n', long = "dry-run")]
        dry_run: bool,

        /// The set of commits to test.
        #[clap(value_parser, default_value = "stack()")]
        revset: Revset,

        /// Options for resolving revset expressions.
        #[clap(flatten)]
        resolve_revset_options: ResolveRevsetOptions,

        /// Show the test output as well.
        #[clap(short = 'v', long = "verbose", action = clap::ArgAction::Count)]
        verbosity: u8,

        /// How to execute the tests.
        #[clap(short = 's', long = "strategy")]
        strategy: Option<TestExecutionStrategy>,

        /// Don't read or write to the cache when executing the test commands.
        #[clap(long = "no-cache")]
        no_cache: bool,

        /// How many jobs to execute in parallel. The value `0` indicates to use all CPUs.
        #[clap(short = 'j', long = "jobs")]
        jobs: Option<usize>,

        /// Options for moving commits.
        #[clap(flatten)]
        move_options: MoveOptions,
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

    let app =
        // Explicitly set the name here, or else clap thinks that the name of the
        // command is `git-branchless-opts` (and that its subcommands are
        // `git-branchless-opts-amend`, etc.).
        Opts::command().name("git-branchless");
    generate_man_page(&man1_dir, "git-branchless", &app)?;
    for subcommand in app.get_subcommands() {
        let subcommand_exe_name = format!("git-branchless-{}", subcommand.get_name());
        generate_man_page(&man1_dir, &subcommand_exe_name, subcommand)?;
    }
    Ok(())
}

fn generate_man_page(man1_dir: &Path, name: &str, command: &ClapCommand) -> std::io::Result<()> {
    let rendered_man_page = {
        let mut buffer = Vec::new();
        clap_mangen::Man::new(command.clone())
            // The rendered man-page command name would be the subcommand only
            // (such as `amend(1)` instead of `git-branchless-amend(1)`), so
            // override the name here. Also, the top-level man-page name will be
            // `git-branchless-opts(1)` instead of `git-branchless(1)`, which is
            // also handled by this call to `.title`.
            .title(name)
            .render(&mut buffer)?;
        buffer
    };
    let output_path = man1_dir.join(format!("{name}.1"));
    std::fs::write(output_path, rendered_man_page)?;
    Ok(())
}

/// Carry out some rewrites on the command-line arguments for uniformity.
///
/// For example, `git-branchless-smartlog` becomes `git-branchless smartlog`,
/// and the `.exe` suffix is removed on Windows. These are necessary for later
/// command-line argument parsing.
pub fn rewrite_args(args: Vec<OsString>) -> Vec<OsString> {
    let first_arg = match args.first() {
        None => return args,
        Some(first_arg) => first_arg.clone(),
    };

    // Don't use `std::env::current_exe`, because it may or may not resolve the
    // symlink. We want to preserve the symlink in our case. See
    // https://doc.rust-lang.org/std/env/fn.current_exe.html#platform-specific-behavior
    let exe_path = PathBuf::from(first_arg);
    let exe_name = match exe_path.file_name().and_then(|arg| arg.to_str()) {
        Some(exe_name) => exe_name,
        None => return args,
    };

    // On Windows, the first argument might be `git-branchless-smartlog.exe`
    // instead of just `git-branchless-smartlog`. Remove the suffix in that
    // case.
    let exe_name = match exe_name.strip_suffix(std::env::consts::EXE_SUFFIX) {
        Some(exe_name) => exe_name,
        None => exe_name,
    };

    let args = match exe_name.strip_prefix("git-branchless-") {
        Some(subcommand) => {
            let mut new_args = vec![OsString::from("git-branchless"), OsString::from(subcommand)];
            new_args.extend(args.into_iter().skip(1));
            new_args
        }
        None => {
            let mut new_args = vec![OsString::from(exe_name)];
            new_args.extend(args.into_iter().skip(1));
            new_args
        }
    };

    // For backward-compatibility, convert calls of the form
    // `git-branchless-hook-X Y Z` into `git-branchless hook X Y Z`.
    let args = match args.as_slice() {
        [first, subcommand, rest @ ..] if exe_name == "git-branchless" => {
            let mut new_args = vec![first.clone()];
            match subcommand
                .to_str()
                .and_then(|arg| arg.strip_prefix("hook-"))
            {
                Some(hook_subcommand) => {
                    new_args.push(OsString::from("hook"));
                    new_args.push(OsString::from(hook_subcommand));
                }
                None => {
                    new_args.push(subcommand.clone());
                }
            }
            new_args.extend(rest.iter().cloned());
            new_args
        }
        other => other.to_vec(),
    };

    args
}

#[cfg(test)]
mod tests {
    use super::rewrite_args;
    use std::ffi::OsString;

    #[test]
    fn test_rewrite_args() {
        assert_eq!(
            rewrite_args(vec![OsString::from("git-branchless")]),
            vec![OsString::from("git-branchless")]
        );
        assert_eq!(
            rewrite_args(vec![OsString::from("git-branchless-smartlog")]),
            vec![OsString::from("git-branchless"), OsString::from("smartlog")]
        );

        // Should only happen on Windows.
        if std::env::consts::EXE_SUFFIX == ".exe" {
            assert_eq!(
                rewrite_args(vec![OsString::from("git-branchless-smartlog.exe")]),
                vec![OsString::from("git-branchless"), OsString::from("smartlog")]
            );
        }

        assert_eq!(
            rewrite_args(vec![
                OsString::from("git-branchless-smartlog"),
                OsString::from("foo"),
                OsString::from("bar")
            ]),
            vec![
                OsString::from("git-branchless"),
                OsString::from("smartlog"),
                OsString::from("foo"),
                OsString::from("bar")
            ]
        );

        assert_eq!(
            rewrite_args(vec![
                OsString::from("git-branchless"),
                OsString::from("hook-post-commit"),
            ]),
            vec![
                OsString::from("git-branchless"),
                OsString::from("hook"),
                OsString::from("post-commit"),
            ]
        );
        assert_eq!(
            rewrite_args(vec![
                OsString::from("git-branchless-hook"),
                OsString::from("post-commit"),
            ]),
            vec![
                OsString::from("git-branchless"),
                OsString::from("hook"),
                OsString::from("post-commit"),
            ]
        );
        assert_eq!(
            rewrite_args(vec![
                OsString::from("git-branchless"),
                OsString::from("hook-post-checkout"),
                OsString::from("3"),
                OsString::from("2"),
                OsString::from("1"),
            ]),
            vec![
                OsString::from("git-branchless"),
                OsString::from("hook"),
                OsString::from("post-checkout"),
                OsString::from("3"),
                OsString::from("2"),
                OsString::from("1"),
            ]
        );
        assert_eq!(
            rewrite_args(vec![
                OsString::from("target/debug/git-branchless"),
                OsString::from("hook-detect-empty-commit"),
                OsString::from("abc123"),
            ]),
            vec![
                OsString::from("git-branchless"),
                OsString::from("hook"),
                OsString::from("detect-empty-commit"),
                OsString::from("abc123"),
            ]
        );
    }
}
