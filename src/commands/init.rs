//! Install any hooks, aliases, etc. to set up `git-branchless` in this repo.

use std::fmt::Write;
use std::io::{stdin, stdout, BufRead, BufReader, Write as WriteIo};
use std::path::{Path, PathBuf};

use console::style;
use eyre::Context;
use tracing::{instrument, warn};

use crate::core::config::get_core_hooks_path;
use crate::git::{Config, ConfigValue, GitRunInfo, GitVersion, Repo};
use crate::tui::Effects;

const ALL_HOOKS: &[(&str, &str)] = &[
    (
        "post-commit",
        r#"
git branchless hook-post-commit "$@"
"#,
    ),
    (
        "post-merge",
        r#"
        git branchless hook-post-merge "$@"
        "#,
    ),
    (
        "post-rewrite",
        r#"
git branchless hook-post-rewrite "$@"
"#,
    ),
    (
        "post-checkout",
        r#"
git branchless hook-post-checkout "$@"
"#,
    ),
    (
        "pre-auto-gc",
        r#"
git branchless hook-pre-auto-gc "$@"
"#,
    ),
    (
        "reference-transaction",
        r#"
# Avoid canceling the reference transaction in the case that `branchless` fails
# for whatever reason.
git branchless hook-reference-transaction "$@" || (
echo 'branchless: Failed to process reference transaction!'
echo 'branchless: Some events (e.g. branch updates) may have been lost.'
echo 'branchless: This is a bug. Please report it.'
)
"#,
    ),
];

const ALL_ALIASES: &[(&str, &str)] = &[
    ("smartlog", "smartlog"),
    ("sl", "smartlog"),
    ("hide", "hide"),
    ("unhide", "unhide"),
    ("prev", "prev"),
    ("next", "next"),
    ("restack", "restack"),
    ("undo", "undo"),
    ("move", "move"),
];

#[derive(Debug)]
enum Hook {
    /// Regular Git hook.
    RegularHook { path: PathBuf },

    /// For Twitter multihooks.
    MultiHook { path: PathBuf },
}

#[instrument]
fn determine_hook_path(repo: &Repo, hook_type: &str) -> eyre::Result<Hook> {
    let multi_hooks_path = repo.get_path().join("hooks_multi");
    let hook = if multi_hooks_path.exists() {
        let path = multi_hooks_path
            .join(format!("{}.d", hook_type))
            .join("00_local_branchless");
        Hook::MultiHook { path }
    } else {
        let hooks_dir = get_core_hooks_path(repo)?;
        let path = hooks_dir.join(hook_type);
        Hook::RegularHook { path }
    };
    Ok(hook)
}

const SHEBANG: &str = "#!/bin/sh";
const UPDATE_MARKER_START: &str = "## START BRANCHLESS CONFIG";
const UPDATE_MARKER_END: &str = "## END BRANCHLESS CONFIG";

fn update_between_lines(lines: &str, updated_lines: &str) -> String {
    let mut new_lines = String::new();
    let mut is_ignoring_lines = false;
    for line in lines.lines() {
        if line == UPDATE_MARKER_START {
            is_ignoring_lines = true;
            new_lines.push_str(UPDATE_MARKER_START);
            new_lines.push('\n');
            new_lines.push_str(updated_lines);
            new_lines.push_str(UPDATE_MARKER_END);
            new_lines.push('\n');
        } else if line == UPDATE_MARKER_END {
            is_ignoring_lines = false;
        } else if !is_ignoring_lines {
            new_lines.push_str(line);
            new_lines.push('\n');
        }
    }
    if is_ignoring_lines {
        warn!("Unterminated branchless config comment in hook");
    }
    new_lines
}

#[instrument]
fn write_script(path: &Path, contents: &str) -> eyre::Result<()> {
    let script_dir = path
        .parent()
        .ok_or_else(|| eyre::eyre!("No parent for dir {:?}", path))?;
    std::fs::create_dir_all(script_dir).wrap_err("Creating script dir")?;

    std::fs::write(path, contents).wrap_err("Writing script contents")?;

    // Setting hook file as executable only supported on Unix systems.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(path).wrap_err("Reading script permissions")?;
        let mut permissions = metadata.permissions();
        let mode = permissions.mode();
        // Set execute bits.
        let mode = mode | 0o111;
        permissions.set_mode(mode);
        std::fs::set_permissions(path, permissions)
            .wrap_err_with(|| format!("Marking {:?} as executable", path))?;
    }

    Ok(())
}

#[instrument]
fn update_hook_contents(hook: &Hook, hook_contents: &str) -> eyre::Result<()> {
    let (hook_path, hook_contents) = match hook {
        Hook::RegularHook { path } => match std::fs::read_to_string(path) {
            Ok(lines) => {
                let lines = update_between_lines(&lines, hook_contents);
                (path, lines)
            }
            Err(ref err) if err.kind() == std::io::ErrorKind::NotFound => {
                let hook_contents = format!(
                    "{}\n{}\n{}\n{}\n",
                    SHEBANG, UPDATE_MARKER_START, hook_contents, UPDATE_MARKER_END
                );
                (path, hook_contents)
            }
            Err(other) => {
                return Err(eyre::eyre!(other));
            }
        },
        Hook::MultiHook { path } => (path, format!("{}\n{}", SHEBANG, hook_contents)),
    };

    write_script(hook_path, &hook_contents).wrap_err("Writing hook script")?;

    Ok(())
}

#[instrument]
fn install_hook(repo: &Repo, hook_type: &str, hook_script: &str) -> eyre::Result<()> {
    let hook = determine_hook_path(repo, hook_type)?;
    update_hook_contents(&hook, hook_script)?;
    Ok(())
}

#[instrument]
fn install_hooks(effects: &Effects, repo: &Repo) -> eyre::Result<()> {
    for (hook_type, hook_script) in ALL_HOOKS {
        writeln!(
            effects.get_output_stream(),
            "Installing hook: {}",
            hook_type
        )?;
        install_hook(repo, hook_type, hook_script)?;
    }
    Ok(())
}

#[instrument]
fn uninstall_hooks(effects: &Effects, repo: &Repo) -> eyre::Result<()> {
    for (hook_type, _hook_script) in ALL_HOOKS {
        writeln!(
            effects.get_output_stream(),
            "Uninstalling hook: {}",
            hook_type
        )?;
        install_hook(
            repo,
            hook_type,
            r#"
# This hook has been uninstalled.
# Run `git branchless init` to reinstall.
"#,
        )?;
    }
    Ok(())
}

/// Determine if we should make an alias of the form `branchless smartlog` or
/// `branchless-smartlog`.
///
/// The form of the alias is important because it determines what command Git
/// tries to look up with `man` when you run e.g. `git smartlog --help`:
///
/// - `branchless smartlog`: invokes `man git-branchless`, which means that the
/// subcommand is not included in the `man` invocation, so it can only show
/// generic help.
/// - `branchless-smartlog`: invokes `man git-branchless-smartlog, so the
/// subcommand is included in the `man` invocation, so it can show more specific
/// help.
fn should_use_wrapped_command_alias() -> bool {
    cfg!(feature = "man-pages")
}

#[instrument]
fn install_alias(repo: &Repo, config: &mut Config, from: &str, to: &str) -> eyre::Result<()> {
    let alias = if should_use_wrapped_command_alias() {
        format!("branchless-{}", to)
    } else {
        format!("branchless {}", to)
    };
    config.set(format!("alias.{}", from), alias)?;
    Ok(())
}

#[instrument]
fn detect_main_branch_name(repo: &Repo) -> eyre::Result<Option<String>> {
    for branch_name in [
        "master",
        "main",
        "mainline",
        "devel",
        "develop",
        "development",
        "trunk",
    ] {
        if repo
            .find_branch(branch_name, git2::BranchType::Local)?
            .is_some()
        {
            return Ok(Some(branch_name.to_string()));
        }
    }
    Ok(None)
}

#[instrument]
fn install_aliases(
    effects: &Effects,
    repo: &mut Repo,
    config: &mut Config,
    git_run_info: &GitRunInfo,
) -> eyre::Result<()> {
    for (from, to) in ALL_ALIASES {
        writeln!(
            effects.get_output_stream(),
            "Installing alias (non-global): git {} -> git branchless {}",
            from,
            to
        )?;
        install_alias(repo, config, from, to)?;
    }

    let version_str = git_run_info
        .run_silent(repo, None, &["version"])
        .wrap_err("Determining Git version")?;
    let version_str = version_str.trim();
    let version: GitVersion = version_str
        .parse()
        .wrap_err_with(|| format!("Parsing Git version string: {}", version_str))?;
    if version < GitVersion(2, 29, 0) {
        write!(
            effects.get_output_stream(),
            "\
{warning_str}: the branchless workflow's `git undo` command requires Git
v2.29 or later, but your Git version is: {version_str}

Some operations, such as branch updates, won't be correctly undone. Other
operations may be undoable. Attempt at your own risk.

Once you upgrade to Git v2.29, run `git branchless init` again. Any work you
do from then on will be correctly undoable.

This only applies to the `git undo` command. Other commands which are part of
the branchless workflow will work properly.
",
            warning_str = style("Warning").yellow().bold(),
            version_str = version_str,
        )?;
    }

    Ok(())
}

#[instrument]
fn uninstall_aliases(effects: &Effects, config: &mut Config) -> eyre::Result<()> {
    for (from, _to) in ALL_ALIASES {
        writeln!(
            effects.get_output_stream(),
            "Uninstalling alias (non-global): git {}",
            from
        )?;
        config
            .remove(&format!("alias.{}", from))
            .wrap_err_with(|| format!("Uninstalling alias {}", from))?;
    }
    Ok(())
}

#[instrument(skip(value))]
fn set_config(
    effects: &Effects,
    config: &mut Config,
    name: &str,
    value: impl Into<ConfigValue>,
) -> eyre::Result<()> {
    fn inner(
        effects: &Effects,
        config: &mut Config,
        name: &str,
        value: ConfigValue,
    ) -> eyre::Result<()> {
        writeln!(
            effects.get_output_stream(),
            "Setting config (non-global): {} = {}",
            name,
            value
        )?;
        config.set(name, value)?;
        Ok(())
    }

    inner(effects, config, name, value.into())
}

#[instrument(skip(r#in))]
fn set_configs(
    r#in: &mut impl BufRead,
    effects: &Effects,
    repo: &Repo,
    config: &mut Config,
) -> eyre::Result<()> {
    let main_branch_name = match detect_main_branch_name(repo)? {
        Some(main_branch_name) => {
            writeln!(
                effects.get_output_stream(),
                "Auto-detected your main branch as: {}",
                console::style(&main_branch_name).bold()
            )?;
            writeln!(
                effects.get_output_stream(),
                "If this is incorrect, run: git config branchless.core.mainBranch <branch>"
            )?;
            main_branch_name
        }
        None => {
            writeln!(
                effects.get_output_stream(),
                "{}",
                console::style("Your main branch name could not be auto-detected!")
                    .yellow()
                    .bold()
            )?;
            writeln!(
                effects.get_output_stream(),
                "Examples of a main branch: master, main, trunk, etc."
            )?;
            writeln!(
                effects.get_output_stream(),
                "See https://github.com/arxanas/git-branchless/wiki/Concepts#main-branch"
            )?;
            write!(
                effects.get_output_stream(),
                "Enter the name of your main branch: "
            )?;
            stdout().flush()?;
            let mut input = String::new();
            r#in.read_line(&mut input)?;
            match input.trim() {
                "" => eyre::bail!("No main branch name provided"),
                main_branch_name => main_branch_name.to_string(),
            }
        }
    };
    set_config(
        effects,
        config,
        "branchless.core.mainBranch",
        main_branch_name,
    )?;
    set_config(effects, config, "advice.detachedHead", false)?;
    Ok(())
}

#[instrument]
fn unset_configs(effects: &Effects, config: &mut Config) -> eyre::Result<()> {
    for key in ["branchless.core.mainBranch", "advice.detachedHead"] {
        writeln!(
            effects.get_output_stream(),
            "Unsetting config (non-global): {}",
            key
        )?;
        config
            .remove(key)
            .wrap_err_with(|| format!("Unsetting config {}", key))?;
    }
    Ok(())
}

/// Initialize `git-branchless` in the current repo.
#[instrument]
pub fn init(effects: &Effects, git_run_info: &GitRunInfo) -> eyre::Result<()> {
    let mut in_ = BufReader::new(stdin());
    let mut repo = Repo::from_current_dir()?;
    let mut config = repo.get_config()?;
    set_configs(&mut in_, effects, &repo, &mut config)?;
    install_hooks(effects, &repo)?;
    install_aliases(effects, &mut repo, &mut config, git_run_info)?;
    writeln!(
        effects.get_output_stream(),
        "{}",
        console::style("Successfully installed git-branchless.")
            .green()
            .bold()
    )?;
    writeln!(
        effects.get_output_stream(),
        "To uninstall, run: {}",
        console::style("git branchless init --uninstall").bold()
    )?;
    Ok(())
}

/// Uninstall `git-branchless` in the current repo.
#[instrument]
pub fn uninstall(effects: &Effects) -> eyre::Result<()> {
    let repo = Repo::from_current_dir()?;
    let mut config = repo.get_config().wrap_err("Getting repo config")?;
    unset_configs(effects, &mut config)?;
    uninstall_hooks(effects, &repo)?;
    uninstall_aliases(effects, &mut config)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{update_between_lines, ALL_ALIASES, UPDATE_MARKER_END, UPDATE_MARKER_START};

    #[test]
    fn test_update_between_lines() {
        let input = format!(
            "\
hello, world
{}
contents 1
{}
goodbye, world
",
            UPDATE_MARKER_START, UPDATE_MARKER_END
        );
        let expected = format!(
            "\
hello, world
{}
contents 2
contents 3
{}
goodbye, world
",
            UPDATE_MARKER_START, UPDATE_MARKER_END
        );

        assert_eq!(
            update_between_lines(
                &input,
                "\
contents 2
contents 3
"
            ),
            expected
        )
    }

    #[test]
    fn test_all_alias_binaries_exist() {
        for (_from, to) in ALL_ALIASES {
            let executable_name = format!("git-branchless-{}", to);

            // For each subcommand that's been aliased, asserts that a binary
            // with the corresponding name exists in `Cargo.toml`. If this test
            // fails, then it may mean that a new binary entry should be added.
            //
            // Note that this check may require a `cargo clean` to clear out any
            // old executables in order to produce deterministic results.
            assert_cmd::cmd::Command::cargo_bin(executable_name)
                .unwrap()
                .arg("--help")
                .assert()
                .success();
        }
    }
}
