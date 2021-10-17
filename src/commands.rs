//! Sub-commands of `git-branchless`.

pub mod gc;
pub mod hide;
pub mod hooks;
pub mod init;
pub mod r#move;
pub mod navigation;
pub mod restack;
pub mod smartlog;
pub mod undo;
pub mod wrap;

use std::any::Any;
use std::convert::TryInto;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::SystemTime;

use clap::Parser;
use eyre::Context;
use itertools::Itertools;
use tracing_chrome::ChromeLayerBuilder;
use tracing_error::ErrorLayer;
use tracing_subscriber::fmt as tracing_fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::core::formatting::Glyphs;
use crate::git::GitRunInfo;
use crate::git::NonZeroOid;
use crate::opts::ColorSetting;
use crate::opts::Command;
use crate::opts::Opts;
use crate::opts::WrappedCommand;
use crate::tui::Effects;

use self::smartlog::SmartlogOptions;

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
    let _tracing_guard = install_tracing();

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

    let path_to_git = std::env::var_os("PATH_TO_GIT").unwrap_or_else(|| OsString::from("git"));
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

    let exit_code = match command {
        Command::Init {
            uninstall: false,
            main_branch_name,
        } => {
            init::init(&effects, &git_run_info, main_branch_name.as_deref())?;
            0
        }

        Command::Init {
            uninstall: true,
            main_branch_name: _,
        } => {
            init::uninstall(&effects)?;
            0
        }

        Command::Smartlog {
            show_hidden_commits,
        } => {
            smartlog::smartlog(
                &effects,
                &SmartlogOptions {
                    show_hidden_commits,
                },
            )?;
            0
        }

        Command::Hide { commits, recursive } => hide::hide(&effects, commits, recursive)?,

        Command::Unhide { commits, recursive } => hide::unhide(&effects, commits, recursive)?,

        Command::Prev { num_commits } => navigation::prev(&effects, &git_run_info, num_commits)?,

        Command::Next {
            num_commits,
            oldest,
            newest,
        } => {
            let towards = match (oldest, newest) {
                (false, false) => None,
                (true, false) => Some(navigation::Towards::Oldest),
                (false, true) => Some(navigation::Towards::Newest),
                (true, true) => eyre::bail!("Both --oldest and --newest were set"),
            };
            navigation::next(&effects, &git_run_info, num_commits, towards)?
        }

        Command::Checkout => navigation::checkout(&effects, &git_run_info)?,

        Command::Move {
            source,
            dest,
            base,
            move_options,
        } => r#move::r#move(&effects, &git_run_info, source, dest, base, &move_options)?,

        Command::Restack {
            commits,
            move_options,
        } => restack::restack(&effects, &git_run_info, commits, &move_options)?,

        Command::Undo => undo::undo(&effects, &git_run_info)?,

        Command::Gc | Command::HookPreAutoGc => {
            gc::gc(&effects)?;
            0
        }

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
            let exit_code = wrap::wrap(&git_run_info, args.as_slice())?;
            exit_code
        }

        Command::HookPostRewrite { rewrite_type } => {
            hooks::hook_post_rewrite(&effects, &git_run_info, &rewrite_type)?;
            0
        }

        Command::HookRegisterExtraPostRewriteHook => {
            hooks::hook_register_extra_post_rewrite_hook()?;
            0
        }

        Command::HookDetectEmptyCommit { old_commit_oid } => {
            let old_commit_oid: NonZeroOid = old_commit_oid.parse()?;
            hooks::hook_drop_commit_if_empty(&effects, old_commit_oid)?;
            0
        }

        Command::HookSkipUpstreamAppliedCommit { commit_oid } => {
            let commit_oid: NonZeroOid = commit_oid.parse()?;
            hooks::hook_skip_upstream_applied_commit(&effects, commit_oid)?;
            0
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
            0
        }

        Command::HookPostCommit => {
            hooks::hook_post_commit(&effects)?;
            0
        }

        Command::HookPostMerge { is_squash_merge } => {
            hooks::hook_post_merge(&effects, is_squash_merge)?;
            0
        }

        Command::HookReferenceTransaction { transaction_state } => {
            hooks::hook_reference_transaction(&effects, &transaction_state)?;
            0
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
fn install_tracing() -> eyre::Result<impl Drop> {
    let (filter_layer, fmt_layer) = match EnvFilter::try_from_default_env() {
        Ok(filter_layer) => {
            let fmt_layer = tracing_fmt::layer()
                .with_span_events(tracing_fmt::format::FmtSpan::CLOSE)
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
        .with(filter_layer)
        .with(fmt_layer)
        .with(profile_layer)
        .try_init()?;

    Ok(flush_guard)
}
