//! Sub-commands of `git-branchless`.

mod amend;
mod bug_report;
mod hide;
mod repair;
mod restack;
mod snapshot;
mod sync;
mod wrap;

use git_branchless_invoke::CommandContext;
use lib::core::rewrite::MergeConflictRemediation;

use lib::util::ExitCode;
use lib::{core::gc, util::EyreExitOr};

use git_branchless_opts::{
    rewrite_args, Command, Opts, ResolveRevsetOptions, SnapshotSubcommand, WrappedCommand,
};
use lib::git::GitRunInfo;

fn command_main(ctx: CommandContext, opts: Opts) -> EyreExitOr<()> {
    let CommandContext {
        effects,
        git_run_info,
    } = ctx.clone();
    let Opts {
        global_args: _,
        command,
    } = opts;

    let exit_code = match command {
        Command::Amend {
            move_options,
            reparent,
        } => amend::amend(
            &effects,
            &git_run_info,
            &ResolveRevsetOptions::default(),
            &move_options,
            reparent,
        )?,

        Command::BugReport => bug_report::bug_report(&effects, &git_run_info)?,

        Command::Difftool(opts) => {
            let result = scm_diff_editor::run(opts);
            match result {
                Ok(()) | Err(scm_diff_editor::Error::Cancelled) => Ok(()),
                Err(err) => {
                    eprintln!("Error: {err}");
                    Err(ExitCode(1))
                }
            }
        }

        Command::Switch { switch_options } => {
            git_branchless_navigation::switch(&effects, &git_run_info, &switch_options)?
        }

        Command::Gc => {
            gc::gc(&effects)?;
            Ok(())
        }

        Command::Hook(args) => git_branchless_hook::command_main(ctx, args)?,

        Command::Hide {
            revsets,
            resolve_revset_options,
            no_delete_branches,
            recursive,
        } => hide::hide(
            &effects,
            &git_run_info,
            revsets,
            &resolve_revset_options,
            no_delete_branches,
            recursive,
        )?,

        Command::Init(args) => git_branchless_init::command_main(ctx, args)?,

        Command::InstallManPages(args) => {
            git_branchless_init::command_install_man_pages(ctx, args)?
        }

        Command::Move {
            source,
            dest,
            base,
            exact,
            resolve_revset_options,
            move_options,
            fixup,
            insert,
        } => git_branchless_move::r#move(
            &effects,
            &git_run_info,
            source,
            dest,
            base,
            exact,
            &resolve_revset_options,
            &move_options,
            fixup,
            insert,
        )?,

        Command::Next {
            traverse_commits_options,
        } => git_branchless_navigation::traverse_commits(
            &effects,
            &git_run_info,
            git_branchless_navigation::Command::Next,
            &traverse_commits_options,
        )?,

        Command::Prev {
            traverse_commits_options,
        } => git_branchless_navigation::traverse_commits(
            &effects,
            &git_run_info,
            git_branchless_navigation::Command::Prev,
            &traverse_commits_options,
        )?,

        Command::Query(args) => git_branchless_query::command_main(ctx, args)?,

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

        Command::Record(args) => git_branchless_record::command_main(ctx, args)?,

        Command::Reword {
            revsets,
            resolve_revset_options,
            messages,
            force_rewrite_public_commits,
            discard,
            commit_to_fixup,
            sign_options,
        } => {
            let messages = if discard {
                git_branchless_reword::InitialCommitMessages::Discard
            } else if let Some(commit_to_fixup) = commit_to_fixup {
                git_branchless_reword::InitialCommitMessages::FixUp(commit_to_fixup)
            } else {
                git_branchless_reword::InitialCommitMessages::Messages(messages)
            };
            git_branchless_reword::reword(
                &effects,
                revsets,
                &resolve_revset_options,
                messages,
                &git_run_info,
                force_rewrite_public_commits,
                sign_options,
            )?
        }

        Command::Smartlog(args) => git_branchless_smartlog::command_main(ctx, args)?,

        Command::Snapshot { subcommand } => match subcommand {
            SnapshotSubcommand::Create => snapshot::create(&effects, &git_run_info)?,
            SnapshotSubcommand::Restore { snapshot_oid } => {
                snapshot::restore(&effects, &git_run_info, snapshot_oid)?
            }
        },

        Command::Submit(args) => git_branchless_submit::command_main(ctx, args)?,

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

        Command::Test(args) => git_branchless_test::command_main(ctx, args)?,

        Command::Undo { interactive, yes } => {
            git_branchless_undo::undo(&effects, &git_run_info, interactive, yes)?
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

    Ok(exit_code)
}

/// Execute the main process and exit with the appropriate exit code.
pub fn main() {
    // Install panic handler.
    color_eyre::install().expect("Could not install panic handler");
    let args: Vec<_> = std::env::args_os().collect();
    let args = rewrite_args(args);
    let exit_code = git_branchless_invoke::do_main_and_drop_locals(command_main, args)
        .expect("A fatal error occurred");
    std::process::exit(exit_code);
}
