//! Sub-commands of `git-branchless`.

mod amend;
mod bug_report;
mod gc;
mod hide;
mod hooks;
mod init;
mod r#move;
mod navigation;
mod query;
mod record;
mod repair;
mod restack;
mod reword;
mod smartlog;
mod snapshot;
mod submit;
mod sync;
mod test;
mod undo;
mod wrap;

use std::any::Any;
use std::convert::TryInto;
use std::ffi::OsString;
use std::fmt::Write;
use std::path::PathBuf;
use std::time::SystemTime;

use clap::Parser;
use cursive::theme::BaseColor;
use cursive::utils::markup::StyledString;
use eyre::Context;
use itertools::Itertools;
use lib::core::rewrite::MergeConflictRemediation;
use lib::git::Repo;
use lib::git::RepoError;
use lib::util::ExitCode;
use tracing::level_filters::LevelFilter;
use tracing_chrome::ChromeLayerBuilder;
use tracing_error::ErrorLayer;
use tracing_subscriber::fmt as tracing_fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::opts::Command;
use crate::opts::Opts;
use crate::opts::ResolveRevsetOptions;
use crate::opts::Revset;
use crate::opts::SnapshotSubcommand;
use crate::opts::WrappedCommand;
use crate::opts::{ColorSetting, TestSubcommand};
use lib::core::config::env_vars::get_path_to_git;
use lib::core::effects::Effects;
use lib::core::formatting::Glyphs;
use lib::git::GitRunInfo;
use lib::git::NonZeroOid;

use self::reword::InitialCommitMessages;
use self::smartlog::SmartlogOptions;
use self::test::RawTestOptions;

fn rewrite_args(args: Vec<OsString>) -> Vec<OsString> {
    let first_arg = match args.first() {
        None => return args,
        Some(first_arg) => first_arg,
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

    match exe_name.strip_prefix("git-branchless-") {
        Some(subcommand) => {
            let mut new_args = vec![OsString::from("git-branchless"), OsString::from(subcommand)];
            new_args.extend(args.into_iter().skip(1));
            new_args
        }
        None => args,
    }
}

/// Wrapper function for `main` to ensure that `Drop` is called for local
/// variables, since `std::process::exit` will skip them.
fn do_main_and_drop_locals() -> eyre::Result<i32> {
    let args = rewrite_args(std::env::args_os().collect_vec());
    let Opts {
        working_directory,
        command,
        color,
    } = Opts::parse_from(args);
    if let Some(working_directory) = working_directory {
        std::env::set_current_dir(&working_directory).wrap_err_with(|| {
            format!(
                "Could not set working directory to: {:?}",
                &working_directory
            )
        })?;
    }

    let path_to_git = get_path_to_git().unwrap_or_else(|_| PathBuf::from("git"));
    let path_to_git = PathBuf::from(&path_to_git);
    let git_run_info = GitRunInfo {
        path_to_git,
        working_directory: std::env::current_dir()?,
        env: std::env::vars_os().collect(),
    };

    let color = match color {
        Some(ColorSetting::Always) => Glyphs::pretty(),
        Some(ColorSetting::Never) => Glyphs::text(),
        Some(ColorSetting::Auto) | None => Glyphs::detect(),
    };
    let effects = Effects::new(color);

    let _tracing_guard = install_tracing(effects.clone());

    if let Some(ExitCode(exit_code)) = check_unsupported_config_options(&effects)? {
        let exit_code: i32 = exit_code.try_into()?;
        return Ok(exit_code);
    }

    let ExitCode(exit_code) = match command {
        Command::Amend { move_options } => amend::amend(
            &effects,
            &git_run_info,
            &ResolveRevsetOptions::default(),
            &move_options,
        )?,

        Command::BugReport => bug_report::bug_report(&effects, &git_run_info)?,

        Command::Switch { switch_options } => {
            navigation::switch(&effects, &git_run_info, &switch_options)?
        }

        Command::Gc | Command::HookPreAutoGc => {
            gc::gc(&effects)?;
            ExitCode(0)
        }

        Command::Hide {
            revsets,
            resolve_revset_options,
            delete_branches,
            recursive,
        } => hide::hide(
            &effects,
            &git_run_info,
            revsets,
            &resolve_revset_options,
            delete_branches,
            recursive,
        )?,

        Command::HookDetectEmptyCommit { old_commit_oid } => {
            let old_commit_oid: NonZeroOid = old_commit_oid.parse()?;
            hooks::hook_drop_commit_if_empty(&effects, old_commit_oid)?;
            ExitCode(0)
        }

        Command::HookPostCheckout {
            previous_commit,
            current_commit,
            is_branch_checkout,
        } => {
            hooks::hook_post_checkout(
                &effects,
                &previous_commit,
                &current_commit,
                is_branch_checkout,
            )?;
            ExitCode(0)
        }

        Command::HookPostCommit => {
            hooks::hook_post_commit(&effects)?;
            ExitCode(0)
        }

        Command::HookPostMerge { is_squash_merge } => {
            hooks::hook_post_merge(&effects, is_squash_merge)?;
            ExitCode(0)
        }

        Command::HookPostRewrite { rewrite_type } => {
            hooks::hook_post_rewrite(&effects, &git_run_info, &rewrite_type)?;
            ExitCode(0)
        }

        Command::HookReferenceTransaction { transaction_state } => {
            hooks::hook_reference_transaction(&effects, &transaction_state)?;
            ExitCode(0)
        }

        Command::HookRegisterExtraPostRewriteHook => {
            hooks::hook_register_extra_post_rewrite_hook()?;
            ExitCode(0)
        }

        Command::HookSkipUpstreamAppliedCommit { commit_oid } => {
            let commit_oid: NonZeroOid = commit_oid.parse()?;
            hooks::hook_skip_upstream_applied_commit(&effects, commit_oid)?;
            ExitCode(0)
        }

        Command::Init {
            uninstall: false,
            main_branch_name,
        } => {
            init::init(&effects, &git_run_info, main_branch_name.as_deref())?;
            ExitCode(0)
        }

        Command::Init {
            uninstall: true,
            main_branch_name: _,
        } => {
            init::uninstall(&effects)?;
            ExitCode(0)
        }

        Command::Move {
            source,
            dest,
            base,
            exact,
            resolve_revset_options,
            move_options,
            insert,
        } => r#move::r#move(
            &effects,
            &git_run_info,
            source,
            dest,
            base,
            exact,
            &resolve_revset_options,
            &move_options,
            insert,
        )?,

        Command::Next {
            traverse_commits_options,
        } => navigation::traverse_commits(
            &effects,
            &git_run_info,
            navigation::Command::Next,
            &traverse_commits_options,
        )?,

        Command::Prev {
            traverse_commits_options,
        } => navigation::traverse_commits(
            &effects,
            &git_run_info,
            navigation::Command::Prev,
            &traverse_commits_options,
        )?,

        Command::Query {
            revset,
            resolve_revset_options,
            show_branches,
            raw,
        } => query::query(
            &effects,
            &git_run_info,
            revset,
            &resolve_revset_options,
            show_branches,
            raw,
        )?,

        Command::Repair { dry_run } => repair::repair(&effects, dry_run)?,

        Command::Restack {
            revsets,
            resolve_revset_options,
            move_options,
        } => restack::restack(
            &effects,
            &git_run_info,
            revsets,
            &resolve_revset_options,
            &move_options,
            MergeConflictRemediation::Retry,
        )?,

        Command::Record {
            message,
            interactive,
            detach,
        } => record::record(&effects, &git_run_info, message, interactive, detach)?,

        Command::Reword {
            revsets,
            resolve_revset_options,
            messages,
            force_rewrite_public_commits,
            discard,
            commit_to_fixup,
        } => {
            let messages = if discard {
                InitialCommitMessages::Discard
            } else if let Some(commit_to_fixup) = commit_to_fixup {
                InitialCommitMessages::FixUp(commit_to_fixup)
            } else {
                InitialCommitMessages::Messages(messages)
            };
            reword::reword(
                &effects,
                revsets,
                &resolve_revset_options,
                messages,
                &git_run_info,
                force_rewrite_public_commits,
            )?
        }

        Command::Smartlog {
            event_id,
            revset,
            resolve_revset_options,
        } => smartlog::smartlog(
            &effects,
            &git_run_info,
            &SmartlogOptions {
                event_id,
                revset: revset.unwrap_or_else(Revset::default_smartlog_revset),
                resolve_revset_options,
            },
        )?,

        Command::Snapshot { subcommand } => match subcommand {
            SnapshotSubcommand::Create => snapshot::create(&effects, &git_run_info)?,
            SnapshotSubcommand::Restore { snapshot_oid } => {
                snapshot::restore(&effects, &git_run_info, snapshot_oid)?
            }
        },

        Command::Submit {
            create,
            revset,
            resolve_revset_options,
        } => submit::submit(
            &effects,
            &git_run_info,
            revset,
            &resolve_revset_options,
            create,
        )?,

        Command::Sync {
            pull,
            move_options,
            revsets,
            resolve_revset_options,
        } => sync::sync(
            &effects,
            &git_run_info,
            pull,
            &move_options,
            revsets,
            &resolve_revset_options,
        )?,

        Command::Test { subcommand } => match subcommand {
            TestSubcommand::Run {
                exec: command,
                command: command_alias,
                revset,
                resolve_revset_options,
                verbosity,
                strategy,
                jobs,
            } => test::run(
                &effects,
                &git_run_info,
                &RawTestOptions {
                    exec: command,
                    command: command_alias,
                    strategy,
                    jobs,
                    verbosity: test::Verbosity::from(verbosity),
                },
                revset,
                &resolve_revset_options,
            )?,

            TestSubcommand::Show {
                exec: command,
                command: command_alias,
                revset,
                resolve_revset_options,
                verbosity,
            } => test::show(
                &effects,
                &RawTestOptions {
                    exec: command,
                    command: command_alias,
                    strategy: None,
                    jobs: None,
                    verbosity: test::Verbosity::from(verbosity),
                },
                revset,
                &resolve_revset_options,
            )?,

            TestSubcommand::Clean {
                revset,
                resolve_revset_options,
            } => test::clean(&effects, revset, &resolve_revset_options)?,
        },

        Command::Undo { interactive, yes } => {
            undo::undo(&effects, &git_run_info, interactive, yes)?
        }

        Command::Unhide {
            revsets,
            resolve_revset_options,
            recursive,
        } => hide::unhide(&effects, revsets, &resolve_revset_options, recursive)?,

        Command::Wrap {
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
            wrap::wrap(&git_run_info, args.as_slice())?
        }
    };

    let exit_code: i32 = exit_code.try_into()?;
    Ok(exit_code)
}

/// Execute the main process and exit with the appropriate exit code.
pub fn main() {
    // Install panic handler.
    color_eyre::install().expect("Could not install panic handler");

    let exit_code = do_main_and_drop_locals().expect("A fatal error occurred");
    std::process::exit(exit_code)
}

#[must_use = "This function returns a guard object to flush traces. Dropping it immediately is probably incorrect. Make sure that the returned value lives until tracing has finished."]
fn install_tracing(effects: Effects) -> eyre::Result<impl Drop> {
    let env_filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .from_env_lossy();
    let fmt_layer = tracing_fmt::layer().with_writer(move || effects.clone().get_error_stream());

    let (profile_layer, flush_guard): (_, Box<dyn Any>) = {
        // We may invoke a hook that calls back into `git-branchless`. In that case,
        // we have to be careful not to write to the same logging file.
        const NESTING_LEVEL_KEY: &str = "RUST_LOGGING_NESTING_LEVEL";
        let nesting_level = match std::env::var(NESTING_LEVEL_KEY) {
            Ok(nesting_level) => nesting_level.parse::<usize>().unwrap_or_default(),
            Err(_) => 0,
        };
        std::env::set_var(NESTING_LEVEL_KEY, (nesting_level + 1).to_string());

        let should_include_function_args = match std::env::var("RUST_PROFILE_INCLUDE_ARGS") {
            Ok(value) if !value.is_empty() => true,
            Ok(_) | Err(_) => false,
        };

        let filename = match std::env::var("RUST_PROFILE") {
            Ok(value) if value == "1" || value == "true" => {
                let filename = format!(
                    "trace-{}.json-{}",
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)?
                        .as_secs(),
                    nesting_level,
                );
                Some(filename)
            }
            Ok(value) if !value.is_empty() => Some(format!("{}-{}", value, nesting_level)),
            Ok(_) | Err(_) => None,
        };

        match filename {
            Some(filename) => {
                let (layer, flush_guard) = ChromeLayerBuilder::new()
                    .file(filename)
                    .include_args(should_include_function_args)
                    .build();
                (Some(layer), Box::new(flush_guard))
            }
            None => {
                struct TrivialDrop;
                (None, Box::new(TrivialDrop))
            }
        }
    };

    tracing_subscriber::registry()
        .with(ErrorLayer::default())
        .with(fmt_layer.with_filter(env_filter))
        .with(profile_layer)
        .try_init()?;

    Ok(flush_guard)
}

fn check_unsupported_config_options(effects: &Effects) -> eyre::Result<Option<ExitCode>> {
    let _repo = match Repo::from_current_dir() {
        Ok(repo) => repo,
        Err(RepoError::UnsupportedExtensionWorktreeConfig(_)) => {
            writeln!(
                effects.get_output_stream(),
                "\
{error}

Usually, this configuration setting is enabled when initializing a sparse
checkout. See https://github.com/arxanas/git-branchless/issues/278 for more
information.

Here are some options:

- To unset the configuration option, run: git config --unset extensions.worktreeConfig
  - This is safe unless you created another worktree also using a sparse checkout.
- Try upgrading to Git v2.36+ and reinitializing your sparse checkout.",
                error = effects.get_glyphs().render(StyledString::styled(
                    "\
Error: the Git configuration setting `extensions.worktreeConfig` is enabled in
this repository. Due to upstream libgit2 limitations, git-branchless does not
support repositories with this configuration option enabled.",
                    BaseColor::Red.light()
                ))?,
            )?;
            return Ok(Some(ExitCode(1)));
        }
        Err(_) => return Ok(None),
    };

    Ok(None)
}

/// Re-exports of internals for testing purposes.
pub mod testing {
    pub use crate::commands::undo::testing as undo;
}
