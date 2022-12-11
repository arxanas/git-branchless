use std::fmt::Write;
use std::time::SystemTime;

use cursive_core::theme::{BaseColor, Effect, Style};
use itertools::{Either, Itertools};
use lazy_static::lazy_static;
use lib::core::dag::{commit_set_to_vec, Dag};
use lib::core::effects::{Effects, OperationType};
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::{Pluralize, StyledStringBuilder};
use lib::core::repo_ext::RepoExt;
use lib::git::{Branch, BranchType, CategorizedReferenceName, ConfigRead, GitRunInfo, Repo};
use lib::util::ExitCode;

use git_branchless_opts::{ResolveRevsetOptions, Revset};
use git_branchless_revset::resolve_commits;

lazy_static! {
    static ref STYLE_PUSHED: Style =
        Style::merge(&[BaseColor::Green.light().into(), Effect::Bold.into()]);
    static ref STYLE_SKIPPED: Style =
        Style::merge(&[BaseColor::Yellow.light().into(), Effect::Bold.into()]);
}

pub fn submit(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    revset: Revset,
    resolve_revset_options: &ResolveRevsetOptions,
    create: bool,
) -> eyre::Result<ExitCode> {
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let now = SystemTime::now();
    let event_tx_id = event_log_db.make_transaction_id(now, "submit")?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let references_snapshot = repo.get_references_snapshot()?;
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let commit_set =
        match resolve_commits(effects, &repo, &mut dag, &[revset], resolve_revset_options) {
            Ok(mut commit_sets) => commit_sets.pop().unwrap(),
            Err(err) => {
                err.describe(effects)?;
                return Ok(ExitCode(1));
            }
        };

    let branches: Vec<Branch> = commit_set_to_vec(&commit_set)?
        .into_iter()
        .flat_map(|commit_oid| references_snapshot.branch_oid_to_names.get(&commit_oid))
        .flatten()
        .filter_map(
            |reference_name| match CategorizedReferenceName::new(reference_name) {
                name @ CategorizedReferenceName::LocalBranch { .. } => name.remove_prefix().ok(),
                CategorizedReferenceName::RemoteBranch { .. }
                | CategorizedReferenceName::OtherRef { .. } => None,
            },
        )
        .map(|branch_name| -> eyre::Result<Branch> {
            let branch = repo.find_branch(&branch_name, BranchType::Local)?;
            let branch =
                branch.ok_or_else(|| eyre::eyre!("Could not look up branch {branch_name:?}"))?;
            Ok(branch)
        })
        .collect::<Result<_, _>>()?;
    let branches_and_remotes: Vec<(Branch, Option<String>)> = branches
        .into_iter()
        .map(|branch| -> eyre::Result<_> {
            let remote_name = branch.get_push_remote_name()?;
            Ok((branch, remote_name))
        })
        .collect::<Result<_, _>>()?;
    let (branches_without_remotes, branches_with_remotes): (Vec<_>, Vec<_>) = branches_and_remotes
        .into_iter()
        .partition_map(|(branch, remote_name)| match remote_name {
            None => Either::Left(branch),
            Some(remote_name) => Either::Right((branch, remote_name)),
        });
    let remotes_to_branches = branches_with_remotes
        .into_iter()
        .map(|(v, k)| (k, v))
        .into_group_map();

    let (created_branches, uncreated_branches) = {
        let mut branch_names: Vec<&str> = branches_without_remotes
            .iter()
            .map(|branch| branch.get_name())
            .collect::<Result<_, _>>()?;
        branch_names.sort_unstable();
        if branches_without_remotes.is_empty() {
            Default::default()
        } else if create {
            let push_remote: String = match get_default_remote(&repo)? {
                Some(push_remote) => push_remote,
                None => {
                    writeln!(
                        effects.get_output_stream(),
                        "\
No upstream repository was associated with {} and no value was
specified for `remote.pushDefault`, so cannot push these branches: {}
Configure a value with: git config remote.pushDefault <remote>
These remotes are available: {}",
                        CategorizedReferenceName::new(
                            &repo.get_main_branch()?.get_reference_name()?,
                        )
                        .friendly_describe(),
                        branch_names.join(", "),
                        repo.get_all_remote_names()?.join(", "),
                    )?;
                    return Ok(ExitCode(1));
                }
            };

            // This will fail if somebody else created the branch on the remote and we don't
            // know about it.
            let mut args = vec!["push", "--set-upstream", &push_remote];
            args.extend(branch_names.iter());
            {
                let (effects, progress) = effects.start_operation(OperationType::PushBranches);
                progress.notify_progress(0, branch_names.len());
                let exit_code = git_run_info.run(&effects, Some(event_tx_id), &args)?;
                if !exit_code.is_success() {
                    return Ok(exit_code);
                }
            }
            (branch_names, Default::default())
        } else {
            (Default::default(), branch_names)
        }
    };

    // TODO: explain why fetching here.
    let remote_names = remotes_to_branches.keys().sorted().collect_vec();
    if !remote_names.is_empty() {
        let remote_args = {
            let mut result = vec!["fetch"];
            for remote_name in &remote_names {
                result.push(remote_name.as_str());
            }
            result
        };
        let exit_code = git_run_info.run(effects, Some(event_tx_id), &remote_args)?;
        if !exit_code.is_success() {
            writeln!(
                effects.get_output_stream(),
                "Failed to fetch from remotes: {}",
                remote_names.into_iter().join(", ")
            )?;
            return Ok(exit_code);
        }
    }

    let (pushed_branches, skipped_branches) = {
        let (effects, progress) = effects.start_operation(OperationType::PushBranches);

        let mut pushed_branches: Vec<&str> = Vec::new();
        let mut skipped_branches: Vec<&str> = Vec::new();
        let total_num_branches = remotes_to_branches
            .values()
            .map(|branches| branches.len())
            .sum();
        progress.notify_progress(0, total_num_branches);
        for (remote_name, branches) in remotes_to_branches
            .iter()
            .sorted_by(|(k1, _v1), (k2, _v2)| k1.cmp(k2))
        {
            let (branches_to_push_names, branches_to_skip_names) = {
                let mut branches_to_push_names = Vec::new();
                let mut branches_to_skip_names = Vec::new();
                for branch in branches {
                    let branch_name = branch.get_name()?;
                    if let Some(upstream_branch) = branch.get_upstream_branch()? {
                        if upstream_branch.get_oid()? == branch.get_oid()? {
                            branches_to_skip_names.push(branch_name);
                            continue;
                        }
                    }
                    branches_to_push_names.push(branch_name);
                }
                branches_to_push_names.sort_unstable();
                branches_to_skip_names.sort_unstable();
                (branches_to_push_names, branches_to_skip_names)
            };
            pushed_branches.extend(branches_to_push_names.iter());
            skipped_branches.extend(branches_to_skip_names.iter());

            if !pushed_branches.is_empty() {
                let mut args = vec!["push", "--force-with-lease", remote_name];
                args.extend(branches_to_push_names.iter());
                let exit_code = git_run_info.run(&effects, Some(event_tx_id), &args)?;
                if !exit_code.is_success() {
                    writeln!(
                        effects.get_output_stream(),
                        "Failed to push branches: {}",
                        branches_to_push_names.into_iter().join(", ")
                    )?;
                    return Ok(exit_code);
                }
            }
            progress.notify_progress_inc(branches.len());
        }
        (pushed_branches, skipped_branches)
    };

    if !created_branches.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "Created {}: {}",
            Pluralize {
                determiner: None,
                amount: created_branches.len(),
                unit: ("branch", "branches")
            },
            created_branches
                .into_iter()
                .map(|branch_name| effects
                    .get_glyphs()
                    .render(
                        StyledStringBuilder::new()
                            .append_styled(branch_name, *STYLE_PUSHED)
                            .build(),
                    )
                    .expect("Rendering branch name"))
                .join(", ")
        )?;
    }
    if !pushed_branches.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "Pushed {}: {}",
            Pluralize {
                determiner: None,
                amount: pushed_branches.len(),
                unit: ("branch", "branches")
            },
            pushed_branches
                .into_iter()
                .map(|branch_name| effects
                    .get_glyphs()
                    .render(
                        StyledStringBuilder::new()
                            .append_styled(branch_name, *STYLE_PUSHED)
                            .build(),
                    )
                    .expect("Rendering branch name"))
                .join(", ")
        )?;
    }
    if !skipped_branches.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "Skipped {} (already up-to-date): {}",
            Pluralize {
                determiner: None,
                amount: skipped_branches.len(),
                unit: ("branch", "branches")
            },
            skipped_branches
                .into_iter()
                .map(|branch_name| effects
                    .get_glyphs()
                    .render(
                        StyledStringBuilder::new()
                            .append_styled(branch_name, *STYLE_SKIPPED)
                            .build(),
                    )
                    .expect("Rendering branch name"))
                .join(", ")
        )?;
    }
    if !uncreated_branches.is_empty() {
        writeln!(
            effects.get_output_stream(),
            "Skipped {} (not yet on remote): {}",
            Pluralize {
                determiner: None,
                amount: uncreated_branches.len(),
                unit: ("branch", "branches")
            },
            uncreated_branches
                .into_iter()
                .map(|branch_name| effects
                    .get_glyphs()
                    .render(
                        StyledStringBuilder::new()
                            .append_styled(branch_name, *STYLE_SKIPPED)
                            .build(),
                    )
                    .expect("Rendering branch name"))
                .join(", ")
        )?;
        writeln!(
            effects.get_output_stream(),
            "\
These branches were skipped because they were not already associated with a remote repository. To
create and push them, retry this operation with the --create option."
        )?;
    }

    Ok(ExitCode(0))
}

fn get_default_remote(repo: &Repo) -> eyre::Result<Option<String>> {
    let main_branch_name = repo.get_main_branch()?.get_reference_name()?;
    match CategorizedReferenceName::new(&main_branch_name) {
        name @ CategorizedReferenceName::LocalBranch { .. } => {
            if let Some(main_branch) =
                repo.find_branch(&name.remove_prefix()?, BranchType::Local)?
            {
                if let Some(remote_name) = main_branch.get_push_remote_name()? {
                    return Ok(Some(remote_name));
                }
            }
        }

        name @ CategorizedReferenceName::RemoteBranch { .. } => {
            let name = name.remove_prefix()?;
            if let Some((remote_name, _reference_name)) = name.split_once('/') {
                return Ok(Some(remote_name.to_owned()));
            }
        }

        CategorizedReferenceName::OtherRef { .. } => {
            // Do nothing.
        }
    }

    let push_default_remote_opt = repo.get_readonly_config()?.get("remote.pushDefault")?;
    Ok(push_default_remote_opt)
}
