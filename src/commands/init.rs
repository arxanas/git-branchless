//! Install any hooks, aliases, etc. to set up `git-branchless` in this repo.

use std::fmt::Write;
use std::io::{stdin, stdout, BufRead, BufReader, Write as WriteIo};
use std::path::PathBuf;

use console::style;
use eyre::Context;
use tracing::{instrument, warn};

use crate::core::config::get_core_hooks_path;
use crate::git::{Config, ConfigValue, GitRunInfo, GitVersion, Repo};
use crate::tui::Output;

const ALL_HOOKS: &[(&str, &str)] = &[
    (
        "post-commit",
        r#"
git branchless hook-post-commit "$@"
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

    let hook_dir = hook_path
        .parent()
        .ok_or_else(|| eyre::eyre!("No parent for dir {:?}", hook_path))?;
    std::fs::create_dir_all(hook_dir)
        .wrap_err_with(|| format!("Creating hook dir {:?}", hook_path))?;
    std::fs::write(hook_path, hook_contents)
        .wrap_err_with(|| format!("Writing hook contents to {:?}", hook_path))?;

    // Setting hook file as executable only supported on Unix systems.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(hook_path)
            .wrap_err_with(|| format!("Reading hook permissions for {:?}", hook_path))?;
        let mut permissions = metadata.permissions();
        let mode = permissions.mode();
        // Set execute bits.
        let mode = mode | 0o111;
        permissions.set_mode(mode);
        std::fs::set_permissions(hook_path, permissions)
            .wrap_err_with(|| format!("Marking {:?} as executable", hook_path))?;
    }

    Ok(())
}

#[instrument]
fn install_hook(repo: &Repo, hook_type: &str, hook_script: &str) -> eyre::Result<()> {
    let hook = determine_hook_path(repo, hook_type)?;
    update_hook_contents(&hook, hook_script)?;
    Ok(())
}

#[instrument]
fn install_hooks(output: &mut Output, repo: &Repo) -> eyre::Result<()> {
    for (hook_type, hook_script) in ALL_HOOKS {
        writeln!(output, "Installing hook: {}", hook_type)?;
        install_hook(repo, hook_type, hook_script)?;
    }
    Ok(())
}

#[instrument]
fn uninstall_hooks(output: &mut Output, repo: &Repo) -> eyre::Result<()> {
    for (hook_type, _hook_script) in ALL_HOOKS {
        writeln!(output, "Uninstalling hook: {}", hook_type)?;
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

#[instrument]
fn install_alias(config: &mut Config, from: &str, to: &str) -> eyre::Result<()> {
    config.set(format!("alias.{}", from), format!("branchless {}", to))?;
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
    output: &mut Output,
    repo: &mut Repo,
    config: &mut Config,
    git_run_info: &GitRunInfo,
) -> eyre::Result<()> {
    for (from, to) in ALL_ALIASES {
        writeln!(
            output,
            "Installing alias (non-global): git {} -> git branchless {}",
            from, to
        )?;
        install_alias(config, from, to)?;
    }

    let version_str = git_run_info
        .run_silent(repo, None, &["version"])
        .wrap_err_with(|| "Determining Git version")?;
    let version_str = version_str.trim();
    let version: GitVersion = version_str
        .parse()
        .wrap_err_with(|| format!("Parsing Git version string: {}", version_str))?;
    if version < GitVersion(2, 29, 0) {
        write!(
            output,
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
fn uninstall_aliases(output: &mut Output, config: &mut Config) -> eyre::Result<()> {
    for (from, _to) in ALL_ALIASES {
        writeln!(output, "Uninstalling alias (non-global): git {}", from)?;
        config
            .remove(&format!("alias.{}", from))
            .wrap_err_with(|| format!("Uninstalling alias {}", from))?;
    }
    Ok(())
}

#[instrument(skip(value))]
fn set_config(
    output: &mut Output,
    config: &mut Config,
    name: &str,
    value: impl Into<ConfigValue>,
) -> eyre::Result<()> {
    let value = value.into();
    writeln!(output, "Setting config (non-global): {} = {}", name, value)?;
    config.set(name, value)?;
    Ok(())
}

#[instrument(skip(r#in))]
fn set_configs(
    r#in: &mut impl BufRead,
    output: &mut Output,
    repo: &Repo,
    config: &mut Config,
) -> eyre::Result<()> {
    let main_branch_name = match detect_main_branch_name(repo)? {
        Some(main_branch_name) => {
            writeln!(
                output,
                "Auto-detected your main branch as: {}",
                console::style(&main_branch_name).bold()
            )?;
            writeln!(
                output,
                "If this is incorrect, run: git config branchless.core.mainBranch <branch>"
            )?;
            main_branch_name
        }
        None => {
            writeln!(
                output,
                "{}",
                console::style("Your main branch name could not be auto-detected!")
                    .yellow()
                    .bold()
            )?;
            writeln!(
                output,
                "Examples of a main branch: master, main, trunk, etc."
            )?;
            writeln!(
                output,
                "See https://github.com/arxanas/git-branchless/wiki/Concepts#main-branch"
            )?;
            write!(output, "Enter the name of your main branch: ")?;
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
        output,
        config,
        "branchless.core.mainBranch",
        main_branch_name,
    )?;
    set_config(output, config, "advice.detachedHead", false)?;
    Ok(())
}

#[instrument]
fn unset_configs(output: &mut Output, config: &mut Config) -> eyre::Result<()> {
    for key in ["branchless.core.mainBranch", "advice.detachedHead"] {
        writeln!(output, "Unsetting config (non-global): {}", key)?;
        config
            .remove(key)
            .wrap_err_with(|| format!("Unsetting config {}", key))?;
    }
    Ok(())
}

/// Initialize `git-branchless` in the current repo.
#[instrument]
pub fn init(output: &mut Output, git_run_info: &GitRunInfo) -> eyre::Result<()> {
    let mut in_ = BufReader::new(stdin());
    let mut repo = Repo::from_current_dir()?;
    let mut config = repo.get_config()?;
    set_configs(&mut in_, output, &repo, &mut config)?;
    install_hooks(output, &repo)?;
    install_aliases(output, &mut repo, &mut config, git_run_info)?;
    writeln!(
        output,
        "{}",
        console::style("Successfully installed git-branchless.")
            .green()
            .bold()
    )?;
    writeln!(
        output,
        "To uninstall, run: {}",
        console::style("git branchless init --uninstall").bold()
    )?;
    Ok(())
}

/// Uninstall `git-branchless` in the current repo.
#[instrument]
pub fn uninstall(output: &mut Output) -> eyre::Result<()> {
    let repo = Repo::from_current_dir()?;
    let mut config = repo.get_config().wrap_err_with(|| "Getting repo config")?;
    unset_configs(output, &mut config)?;
    uninstall_hooks(output, &repo)?;
    uninstall_aliases(output, &mut config)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{update_between_lines, UPDATE_MARKER_END, UPDATE_MARKER_START};

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
}
