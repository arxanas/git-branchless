use std::any::Any;
use std::convert::TryInto;
use std::fmt::Write;
use std::path::PathBuf;
use std::time::SystemTime;

use clap::{CommandFactory, FromArgMatches, Parser};
use cursive_core::theme::BaseColor;
use cursive_core::utils::markup::StyledString;
use eyre::Context;
use git_branchless_opts::{ColorSetting, GlobalArgs};
use lib::core::config::env_vars::get_path_to_git;
use lib::core::effects::Effects;
use lib::core::formatting::Glyphs;
use lib::git::GitRunInfo;
use lib::git::{Repo, RepoError};
use lib::util::ExitCode;
use tracing::level_filters::LevelFilter;
use tracing_chrome::ChromeLayerBuilder;
use tracing_error::ErrorLayer;
use tracing_subscriber::fmt as tracing_fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

#[derive(Clone, Debug)]
pub struct CommandContext {
    pub effects: Effects,
    pub git_run_info: GitRunInfo,
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
            Ok(value) if !value.is_empty() => Some(format!("{value}-{nesting_level}")),
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

/// Wrapper function for `main` to ensure that `Drop` is called for local
/// variables, since `std::process::exit` will skip them.
fn do_main_and_drop_locals<T: Parser>(
    f: impl Fn(CommandContext, T) -> eyre::Result<ExitCode>,
) -> eyre::Result<i32> {
    let args: Vec<_> = std::env::args_os().collect();
    let command = GlobalArgs::command();
    let matches = command.ignore_errors(true).get_matches_from(&args);
    let GlobalArgs {
        working_directory,
        color,
    } = GlobalArgs::from_arg_matches(&matches)
        .map_err(|err| eyre::eyre!("Could not parse global arguments: {err}"))?;
    let command_args = T::parse_from(args);

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

    let ctx = CommandContext {
        effects,
        git_run_info,
    };
    let ExitCode(exit_code) = f(ctx, command_args)?;
    let exit_code: i32 = exit_code.try_into()?;
    Ok(exit_code)
}

pub fn invoke_subcommand_main<T: Parser>(
    f: impl Fn(CommandContext, T) -> eyre::Result<ExitCode>,
) -> ! {
    // Install panic handler.
    color_eyre::install().expect("Could not install panic handler");
    let exit_code = do_main_and_drop_locals(f).expect("A fatal error occurred");
    std::process::exit(exit_code);
}
