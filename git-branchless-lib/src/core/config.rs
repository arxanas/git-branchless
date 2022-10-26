//! Accesses repo-specific configuration.

use std::ffi::OsString;
use std::fmt::Write;
use std::path::PathBuf;

use cursive::theme::{BaseColor, Effect, Style};
use cursive::utils::markup::StyledString;
use eyre::Context;
use tracing::{instrument, warn};

use crate::core::formatting::StyledStringBuilder;
use crate::git::{ConfigRead, GitRunInfo, GitRunOpts, Repo};

use super::effects::Effects;

/// Get the path where Git hooks are stored on disk.
#[instrument]
pub fn get_core_hooks_path(repo: &Repo) -> eyre::Result<PathBuf> {
    repo.get_readonly_config()?
        .get_or_else("core.hooksPath", || repo.get_path().join("hooks"))
}

/// Get the configured name of the main branch.
///
/// The following config values are resolved, in order. The first valid value is returned.
/// - branchless.core.mainBranch
/// - (deprecated) branchless.mainBranch
/// - init.defaultBranch
/// - finally, default to "master"
#[instrument]
pub fn get_main_branch_name(repo: &Repo) -> eyre::Result<String> {
    let config = repo.get_readonly_config()?;

    if let Some(branch_name) = config.get("branchless.core.mainBranch")? {
        return Ok(branch_name);
    }

    if let Some(branch_name) = config.get("branchless.mainBranch")? {
        return Ok(branch_name);
    }

    if let Some(branch_name) = get_default_branch_name(repo)? {
        return Ok(branch_name);
    }

    Ok("master".to_string())
}

/// If `true`, switch to the branch associated with a target commit instead of
/// the commit directly.
///
/// The switch will only occur if it is the only branch on the target commit.
#[instrument]
pub fn get_auto_switch_branches(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.navigation.autoSwitchBranches", true)
}

/// Get the default comment character.
#[instrument]
pub fn get_comment_char(repo: &Repo) -> eyre::Result<char> {
    let from_config: Option<String> = repo.get_readonly_config()?.get("core.commentChar")?;
    let comment_char = match from_config {
        // Note that git also allows `core.commentChar="auto"`, which we do not currently support.
        Some(comment_char) => comment_char.chars().next().unwrap(),
        None => char::from(git2::DEFAULT_COMMENT_CHAR.unwrap()),
    };
    Ok(comment_char)
}

/// Get the commit template message, if any.
#[instrument]
pub fn get_commit_template(repo: &Repo) -> eyre::Result<Option<String>> {
    let commit_template_path: Option<String> =
        repo.get_readonly_config()?.get("commit.template")?;
    let commit_template_path = match commit_template_path {
        Some(commit_template_path) => PathBuf::from(commit_template_path),
        None => return Ok(None),
    };

    let commit_template_path = if commit_template_path.is_relative() {
        match repo.get_working_copy_path() {
            Some(root) => root.join(commit_template_path),
            None => {
                warn!(?commit_template_path, "Commit template path was relative, but this repository does not have a working copy");
                return Ok(None);
            }
        }
    } else {
        commit_template_path
    };

    match std::fs::read_to_string(&commit_template_path) {
        Ok(contents) => Ok(Some(contents)),
        Err(e) => {
            warn!(?e, ?commit_template_path, "Could not read commit template");
            Ok(None)
        }
    }
}

/// Get the default init branch name.
#[instrument]
pub fn get_default_branch_name(repo: &Repo) -> eyre::Result<Option<String>> {
    let config = repo.get_readonly_config()?;
    let default_branch_name: Option<String> = config.get("init.defaultBranch")?;
    Ok(default_branch_name)
}

/// Get the configured editor, if any.
///
/// Because this is primarily intended for use w/ dialoguer::Editor, and it already considers
/// several environment variables, we only need to consider git-specific config options: the
/// `$GIT_EDITOR` environment var and the `core.editor` config setting. We do so in that order to
/// match how git resolves the editor to use.
///
/// FMI see https://git-scm.com/docs/git-var#Documentation/git-var.txt-GITEDITOR
#[instrument]
pub fn get_editor(git_run_info: &GitRunInfo, repo: &Repo) -> eyre::Result<Option<OsString>> {
    if let Ok(result) =
        git_run_info.run_silent(repo, None, &["var", "GIT_EDITOR"], GitRunOpts::default())
    {
        if result.exit_code.is_success() {
            let editor =
                std::str::from_utf8(&result.stdout).context("Decoding git var output as UTF-8")?;
            let editor = editor.trim_end();
            let editor = OsString::from(editor);
            return Ok(Some(editor));
        } else {
            warn!(?result, "`git var` invocation failed");
        }
    }

    let editor = std::env::var_os("GIT_EDITOR");
    if editor != None {
        return Ok(editor);
    }

    let config = repo.get_readonly_config()?;
    let editor: Option<String> = config.get("core.editor")?;
    match editor {
        Some(editor) => Ok(Some(editor.into())),
        None => Ok(None),
    }
}

/// If `true`, create working copy snapshots automatically after certain
/// operations.
#[instrument]
pub fn get_undo_create_snapshots(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.undo.createSnapshots", true)
}

/// If `true`, when restacking a commit, do not update its timestamp to the
/// current time.
#[instrument]
pub fn get_restack_preserve_timestamps(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.restack.preserveTimestamps", false)
}

/// If `true`, when advancing to a "next" commit, prompt interactively to
/// if there is ambiguity in which commit to advance to.
#[instrument]
pub fn get_next_interactive(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.next.interactive", false)
}

/// If `true`, show branches pointing to each commit in the smartlog.
#[instrument]
pub fn get_commit_descriptors_branches(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.commitDescriptors.branches", true)
}

/// If `true`, show associated Phabricator commits in the smartlog.
#[instrument]
pub fn get_commit_descriptors_differential_revision(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.commitDescriptors.differentialRevision", true)
}

/// If `true`, show the age of each commit in the smartlog.
#[instrument]
pub fn get_commit_descriptors_relative_time(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.commitDescriptors.relativeTime", true)
}

/// Config key for `get_restack_warn_abandoned`.
pub const RESTACK_WARN_ABANDONED_CONFIG_KEY: &str = "branchless.restack.warnAbandoned";

/// Possible hint types.
#[derive(Clone, Debug)]
pub enum Hint {
    /// Suggest omitting arguments when they would default to `HEAD`.
    MoveImplicitHeadArgument,

    /// Suggest running `git restack` when a commit is abandoned as part of a `rewrite` event.
    RestackWarnAbandoned,

    /// Suggest running `git restack` when the smartlog prints an abandoned commit.
    SmartlogFixAbandoned,

    /// Suggest showing more output with `git test show` using `--verbose`.
    TestShowVerbose,
}

impl Hint {
    fn get_config_key(&self) -> &'static str {
        match self {
            Hint::MoveImplicitHeadArgument => "branchless.hint.moveImplicitHeadArgument",
            Hint::RestackWarnAbandoned => "branchless.hint.restackWarnAbandoned",
            Hint::SmartlogFixAbandoned => "branchless.hint.smartlogFixAbandoned",
            Hint::TestShowVerbose => "branchless.hint.testShowVerbose",
        }
    }
}

/// Determine if a given hint is enabled.
pub fn get_hint_enabled(repo: &Repo, hint: Hint) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or(hint.get_config_key(), true)
}

/// Render the leading colored "hint" text for use in messaging.
pub fn get_hint_string() -> StyledString {
    StyledStringBuilder::new()
        .append_styled(
            "hint",
            Style::merge(&[BaseColor::Blue.dark().into(), Effect::Bold.into()]),
        )
        .build()
}

/// Print instructions explaining how to disable a given hint.
pub fn print_hint_suppression_notice(effects: &Effects, hint: Hint) -> eyre::Result<()> {
    writeln!(
        effects.get_output_stream(),
        "{}: disable this hint by running: git config --global {} false",
        effects.get_glyphs().render(get_hint_string())?,
        hint.get_config_key(),
    )?;
    Ok(())
}

/// Environment variables which affect the functioning of `git-branchless`.
pub mod env_vars {
    use std::path::PathBuf;

    use tracing::instrument;

    /// Path to the Git executable to shell out to as a subprocess when
    /// appropriate. This may be set during tests.
    pub const TEST_GIT: &str = "TEST_GIT";

    /// "Path to wherever your core Git programs are installed". You can find
    /// the default value by running `git --exec-path`.
    ///
    /// See <https://git-scm.com/docs/git#Documentation/git.txt---exec-pathltpathgt>.
    pub const TEST_GIT_EXEC_PATH: &str = "TEST_GIT_EXEC_PATH";

    /// Get the path to the Git executable for testing.
    #[instrument]
    pub fn get_path_to_git() -> eyre::Result<PathBuf> {
        let path_to_git = std::env::var_os(TEST_GIT).ok_or_else(|| {
            eyre::eyre!(
                "No path to Git executable was set. \
Try running as: `{0}=$(which git) cargo test ...` \
or set `env.{0}` in your `config.toml` \
(see https://doc.rust-lang.org/cargo/reference/config.html)",
                TEST_GIT,
            )
        })?;
        let path_to_git = PathBuf::from(&path_to_git);
        Ok(path_to_git)
    }

    /// Get the `GIT_EXEC_PATH` environment variable for testing.
    #[instrument]
    pub fn get_git_exec_path() -> eyre::Result<PathBuf> {
        let git_exec_path = std::env::var_os(TEST_GIT_EXEC_PATH).ok_or_else(|| {
            eyre::eyre!(
                "No Git exec path was set. \
Try running as: `{0}=$(git --exec-path) cargo test ...` \
or set `env.{0}` in your `config.toml` \
(see https://doc.rust-lang.org/cargo/reference/config.html)",
                TEST_GIT_EXEC_PATH,
            )
        })?;
        let git_exec_path = PathBuf::from(&git_exec_path);
        Ok(git_exec_path)
    }
}
