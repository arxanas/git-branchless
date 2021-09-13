use std::convert::TryInto;
use std::ffi::OsString;
use std::path::PathBuf;

use branchless::commands::wrap;
use branchless::core::formatting::Glyphs;
use branchless::git::{GitRunInfo, NonZeroOid};
use branchless::tui::Effects;
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

        /// Only attempt to perform an in-memory rebase. If it fails, do not
        /// attempt an on-disk rebase.
        #[structopt(long = "--in-memory", conflicts_with = "force_on_disk")]
        force_in_memory: bool,

        /// Skip attempting to use an in-memory rebase, and try an
        /// on-disk rebase directly.
        #[structopt(long = "--on-disk")]
        force_on_disk: bool,

        /// Debugging option. Print the constraints used to create the rebase
        /// plan before executing it.
        #[structopt(long = "--debug-dump-rebase-constraints")]
        dump_rebase_constraints: bool,

        /// Debugging option. Print the rebase plan that will be executed before
        /// executing it.
        #[structopt(long = "--debug-dump-rebase-plan")]
        dump_rebase_plan: bool,
    },

    /// Fix up commits abandoned by a previous rewrite operation.
    Restack {
        /// The IDs of the abandoned commits whose descendants should be
        /// restacked. If not provided, all abandoned commits are restacked.
        commits: Vec<String>,

        /// Only attempt to perform an in-memory rebase. If it fails, do not
        /// attempt an on-disk rebase.
        #[structopt(long = "--in-memory", conflicts_with = "force_on_disk")]
        force_in_memory: bool,

        /// Skip attempting to use an in-memory rebase, and try an
        /// on-disk rebase directly.
        #[structopt(long = "--on-disk")]
        force_on_disk: bool,

        /// Debugging option. Print the constraints used to create the rebase
        /// plan before executing it.
        #[structopt(long = "--debug-dump-rebase-constraints")]
        dump_rebase_constraints: bool,

        /// Debugging option. Print the rebase plan that will be executed before
        /// executing it.
        #[structopt(long = "--debug-dump-rebase-plan")]
        dump_rebase_plan: bool,
    },

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
    HookRegisterExtraPostRewriteHook,

    /// Internal use.
    HookDetectEmptyCommit { old_commit_oid: NonZeroOid },

    /// Internal use.
    HookSkipUpstreamAppliedCommit { commit_oid: NonZeroOid },

    /// Internal use.
    HookPostCheckout {
        previous_commit: String,
        current_commit: String,
        is_branch_checkout: isize,
    },

    /// Internal use.
    HookPostCommit,

    /// Internal use.
    HookPostMerge { is_squash_merge: isize },

    /// Internal use.
    HookReferenceTransaction { transaction_state: String },
}

/// Wrapper function for `main` to ensure that `Drop` is called for local
/// variables, since `std::process::exit` will skip them.
fn do_main_and_drop_locals() -> eyre::Result<i32> {
    color_eyre::install()?;
    let _tracing_guard = install_tracing();

    let opts = Opts::from_args();
    let path_to_git = std::env::var_os("PATH_TO_GIT").unwrap_or_else(|| OsString::from("git"));
    let path_to_git = PathBuf::from(&path_to_git);
    let git_run_info = GitRunInfo {
        path_to_git,
        working_directory: std::env::current_dir()?,
        env: std::env::vars_os().collect(),
    };
    let effects = Effects::new(Glyphs::detect());

    let exit_code = match opts {
        Opts::Init { uninstall: false } => {
            branchless::commands::init::init(&effects, &git_run_info)?;
            0
        }

        Opts::Init { uninstall: true } => {
            branchless::commands::init::uninstall(&effects)?;
            0
        }

        Opts::Smartlog => {
            branchless::commands::smartlog::smartlog(&effects)?;
            0
        }

        Opts::Hide { commits, recursive } => {
            branchless::commands::hide::hide(&effects, commits, recursive)?
        }

        Opts::Unhide { commits, recursive } => {
            branchless::commands::hide::unhide(&effects, commits, recursive)?
        }

        Opts::Prev { num_commits } => {
            branchless::commands::navigation::prev(&effects, &git_run_info, num_commits)?
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
                (true, true) => eyre::bail!("Both --oldest and --newest were set"),
            };
            branchless::commands::navigation::next(&effects, &git_run_info, num_commits, towards)?
        }

        Opts::Move {
            source,
            dest,
            base,
            force_in_memory,
            force_on_disk,
            dump_rebase_constraints,
            dump_rebase_plan,
        } => branchless::commands::r#move::r#move(
            &effects,
            &git_run_info,
            source,
            dest,
            base,
            force_in_memory,
            force_on_disk,
            dump_rebase_constraints,
            dump_rebase_plan,
        )?,

        Opts::Restack {
            commits,
            force_in_memory,
            force_on_disk,
            dump_rebase_constraints,
            dump_rebase_plan,
        } => branchless::commands::restack::restack(
            &effects,
            &git_run_info,
            commits,
            force_in_memory,
            force_on_disk,
            dump_rebase_constraints,
            dump_rebase_plan,
        )?,

        Opts::Undo => branchless::commands::undo::undo(&effects, &git_run_info)?,

        Opts::Gc | Opts::HookPreAutoGc => {
            branchless::commands::gc::gc(&effects)?;
            0
        }

        Opts::Wrap {
            git_executable: explicit_git_executable,
            command: WrappedCommand::WrappedCommand(args),
        } => {
            let git_run_info = match explicit_git_executable {
                Some(path_to_git) => GitRunInfo {
                    path_to_git,
                    ..git_run_info
                },
                None => git_run_info,
            };
            let exit_code = wrap::wrap(&git_run_info, args.as_slice())?;
            exit_code
        }

        Opts::HookPostRewrite { rewrite_type } => {
            branchless::commands::hooks::hook_post_rewrite(&effects, &git_run_info, &rewrite_type)?;
            0
        }

        Opts::HookRegisterExtraPostRewriteHook => {
            branchless::commands::hooks::hook_register_extra_post_rewrite_hook()?;
            0
        }

        Opts::HookDetectEmptyCommit { old_commit_oid } => {
            branchless::commands::hooks::hook_drop_commit_if_empty(&effects, old_commit_oid)?;
            0
        }

        Opts::HookSkipUpstreamAppliedCommit { commit_oid } => {
            branchless::commands::hooks::hook_skip_upstream_applied_commit(&effects, commit_oid)?;
            0
        }

        Opts::HookPostCheckout {
            previous_commit,
            current_commit,
            is_branch_checkout,
        } => {
            branchless::commands::hooks::hook_post_checkout(
                &effects,
                &previous_commit,
                &current_commit,
                is_branch_checkout,
            )?;
            0
        }

        Opts::HookPostCommit => {
            branchless::commands::hooks::hook_post_commit(&effects)?;
            0
        }

        Opts::HookPostMerge { is_squash_merge } => {
            branchless::commands::hooks::hook_post_merge(&effects, is_squash_merge)?;
            0
        }

        Opts::HookReferenceTransaction { transaction_state } => {
            branchless::commands::hooks::hook_reference_transaction(&effects, &transaction_state)?;
            0
        }
    };

    let exit_code: i32 = exit_code.try_into()?;
    Ok(exit_code)
}

fn main() -> eyre::Result<()> {
    let exit_code = do_main_and_drop_locals()?;
    std::process::exit(exit_code)
}

#[must_use = "This function returns a guard object to flush traces. Dropping it immediately is probably incorrect. Make sure that the returned value lives until tracing has finished."]
fn install_tracing() -> Box<dyn Drop> {
    // From https://github.com/yaahc/color-eyre/blob/07b9f0351544e2b07fcd173dc1fc602a7fc8bb6b/examples/usage.rs
    // Licensed under MIT.
    use tracing_chrome::ChromeLayerBuilder;
    use tracing_error::ErrorLayer;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{fmt, EnvFilter};

    let (filter_layer, fmt_layer) = match EnvFilter::try_from_default_env() {
        Ok(filter_layer) => {
            let fmt_layer = fmt::layer()
                .with_span_events(fmt::format::FmtSpan::CLOSE)
                .with_target(false);
            (Some(filter_layer), Some(fmt_layer))
        }
        Err(_) => {
            // We would like the filter layer to apply *only* to the formatting
            // layer. That way, the logging output is suppressed, but we still
            // get spantraces for use with `color-eyre`. However, it's currently
            // not possible (?), at least not without writing some a custom
            // subscriber. See https://github.com/tokio-rs/tracing/pull/1523
            //
            // The workaround is to only display logging messages if `RUST_LOG`
            // is set (which is unfortunate, because we'll miss out on
            // `WARN`-level messages by default).
            (None, None)
        }
    };

    let (profile_layer, profile_layer_guard) = match std::env::var("RUST_PROFILE") {
        Ok(value) if value == "1" || value == "true" => {
            let (chrome_layer, chrome_layer_guard) = ChromeLayerBuilder::new().build();
            (Some(chrome_layer), Some(chrome_layer_guard))
        }
        Ok(value) if !value.is_empty() => {
            let (chrome_layer, chrome_layer_guard) = ChromeLayerBuilder::new().file(value).build();
            (Some(chrome_layer), Some(chrome_layer_guard))
        }
        Ok(_) | Err(_) => (None, None),
    };

    tracing_subscriber::registry()
        .with(ErrorLayer::default())
        .with(filter_layer)
        .with(fmt_layer)
        .with(profile_layer)
        .init();

    match profile_layer_guard {
        Some(profile_layer_guard) => Box::new(profile_layer_guard),
        None => {
            struct TrivialDrop;
            impl Drop for TrivialDrop {
                fn drop(&mut self) {
                    // Do nothing.
                }
            }
            Box::new(TrivialDrop)
        }
    }
}
