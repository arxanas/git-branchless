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
use super::eventlog::EventTransactionId;

/// Get the expected hooks dir inside `.git`, assuming that the user has not
/// overridden it.
#[instrument]
pub fn get_default_hooks_dir(repo: &Repo) -> eyre::Result<PathBuf> {
    let parent_repo = repo.open_worktree_parent_repo()?;
    let repo = parent_repo.as_ref().unwrap_or(repo);
    Ok(repo.get_path().join("hooks"))
}

/// Get the path where the main worktree's Git hooks are stored on disk.
///
/// Git hooks live at `$GIT_DIR/hooks` by default, which means that they will be
/// different per wortkree. Most people, when creating a new worktree, will not
/// also reinstall hooks or reinitialize git-branchless in that worktree, so we
/// instead look up hooks for the main worktree, which is most likely to have them
/// installed.
///
/// This could in theory cause problems for users who have different
/// per-worktree hooks.
#[instrument]
pub fn get_main_worktree_hooks_dir(
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_tx_id: Option<EventTransactionId>,
) -> eyre::Result<PathBuf> {
    let result = git_run_info
        .run_silent(
            repo,
            event_tx_id,
            &["config", "--type", "path", "core.hooksPath"],
            GitRunOpts {
                treat_git_failure_as_error: false,
                ..Default::default()
            },
        )
        .context("Reading core.hooksPath")?;
    let hooks_path = if result.exit_code.is_success() {
        let path = String::from_utf8(result.stdout)
            .context("Decoding git config output for hooks path")?;
        PathBuf::from(path.strip_suffix('\n').unwrap_or(&path))
    } else {
        get_default_hooks_dir(repo)?
    };
    Ok(hooks_path)
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

/// The default smartlog revset to render. This will be used when running `git
/// smartlog` with no arguments, and also when the smartlog is rendered
/// automatically as part of some commands like `git next`/`git prev`.
#[instrument]
pub fn get_smartlog_default_revset(repo: &Repo) -> eyre::Result<String> {
    repo.get_readonly_config()?
        .get_or_else("branchless.smartlog.defaultRevset", || {
            "((draft() | branches() | @) % main()) | branches() | @".to_string()
        })
}

/// Whether to reverse the smartlog direction by default
#[instrument]
pub fn get_smartlog_reverse(repo: &Repo) -> eyre::Result<bool> {
    repo.get_readonly_config()?
        .get_or("branchless.smartlog.reverse", false)
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
                warn!(
                    ?commit_template_path,
                    "Commit template path was relative, but this repository does not have a working copy"
                );
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
/// FMI see <https://git-scm.com/docs/git-var#Documentation/git-var.txt-GITEDITOR>
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
    if editor.is_some() {
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
    /// Suggest running `git add` on skipped, untracked files, which are never
    /// automatically reconsidered for tracking.
    AddSkippedFiles,

    /// Suggest running `git test clean` in order to clean cached test results.
    CleanCachedTestResults,

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
            Hint::AddSkippedFiles => "branchless.hint.addSkippedFiles",
            Hint::CleanCachedTestResults => "branchless.hint.cleanCachedTestResults",
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

    /// Specifies `git-branchless` subcommands to invoke directly.
    ///
    /// For example, `TEST_SEPARATE_COMMAND_BINARIES=init test`, this function
    /// would try to run `git-branchless-init` instead of `git-branchless init`
    /// and `git-branchless-test` instead of `git-branchless test`.
    ///
    /// Why? The `git test` command is implemented in its own
    /// `git-branchless-test` binary. It's slow to include it in
    /// `git-branchless` itself because we have to relink the entire
    /// `git-branchless` binary whenever just a portion of `git-branchless-test`
    /// changes, which leads to slow incremental builds and iteration times.
    ///
    /// Instead, we can assume that its dependency commands like `git branchless
    /// init` don't change when we're incrementally rebuilding the tests, and we
    /// can try to build and use the existing `git-branchless-init` binary on
    /// disk, which shouldn't change when we make changes to
    /// `git-branchless-test`.
    ///
    /// Common dependency binaries like `git-branchless-init` should be built
    /// with the `cargo build -p git-branchless-init` before running incremental
    /// tests. (Ideally, this would happen automatically by marking the binaries
    /// as dependencies and having Cargo build them, but that's not implemented;
    /// see <https://github.com/rust-lang/cargo/issues/4316> for more details).
    ///
    /// If there *is* a change to `git-branchless-init`, and it hasn't been
    /// built, and the test is rerun, then it might fail in an unusual way
    /// because it's invoking the wrong version of the `git-branchless-init`
    /// code. This can be fixed by building the necessary dependency binaries
    /// manually.
    pub const TEST_SEPARATE_COMMAND_BINARIES: &str = "TEST_SEPARATE_COMMAND_BINARIES";

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

    /// Determine whether the specified binary should be run separately. See
    /// [`TEST_SEPARATE_COMMAND_BINARIES`] for more details.
    #[instrument]
    pub fn should_use_separate_command_binary(program: &str) -> bool {
        let values = match std::env::var("TEST_SEPARATE_COMMAND_BINARIES") {
            Ok(value) => value,
            Err(_) => return false,
        };
        let program = program.strip_prefix("git-branchless-").unwrap_or(program);
        values
            .split_ascii_whitespace()
            .any(|value| value.strip_prefix("git-branchless-").unwrap_or(value) == program)
    }
}
