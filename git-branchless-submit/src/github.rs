//! GitHub backend for submitting patch stacks.

use std::collections::HashMap;
use std::fmt::Write;
use std::process::Command;
use std::process::Stdio;
use std::time::SystemTime;

use eyre::Context;
use itertools::Itertools;
use lib::core::dag::CommitSet;
use lib::core::dag::Dag;
use lib::core::effects::Effects;
use lib::core::eventlog::EventLogDb;
use lib::core::repo_ext::RepoExt;
use lib::git::BranchType;
use lib::git::GitRunInfo;
use lib::git::SerializedNonZeroOid;
use lib::git::{NonZeroOid, Repo};
use lib::try_exit_code;
use lib::util::ExitCode;
use lib::util::EyreExitOr;

use serde::Deserialize;
use tracing::debug;
use tracing::instrument;

use crate::branch_forge::BranchForge;
use crate::SubmitStatus;
use crate::{CommitStatus, CreateStatus, Forge, SubmitOptions};

#[derive(Debug, Deserialize)]
struct PullRequestInfo {
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "headRefOid")]
    head_ref_oid: SerializedNonZeroOid,
}

/// The [GitHub](https://en.wikipedia.org/wiki/GitHub) code hosting platform.
/// This forge integrates specifically with the `gh` command-line utility.
#[allow(missing_docs)]
#[derive(Debug)]
pub struct GithubForge<'a> {
    pub effects: &'a Effects,
    pub git_run_info: &'a GitRunInfo,
    pub repo: &'a Repo,
    pub event_log_db: &'a EventLogDb<'a>,
    pub dag: &'a Dag,
}

impl Forge for GithubForge<'_> {
    #[instrument]
    fn query_status(
        &mut self,
        commit_set: CommitSet,
    ) -> EyreExitOr<HashMap<NonZeroOid, CommitStatus>> {
        let output = try_exit_code!(self.run_gh(&[
            "pr",
            "list",
            "--author",
            "@me",
            "--json",
            "headRefName,headRefOid",
        ])?);
        let pr_infos: Vec<PullRequestInfo> = serde_json::from_slice(&output)
            .with_context(|| format!("Deserializing output from gh pr list"))?;
        let remote_branch_name_to_pr_info: HashMap<String, PullRequestInfo> = pr_infos
            .into_iter()
            .map(|item| (item.head_ref_name.clone(), item))
            .collect();

        let mut result = HashMap::new();
        for branch in self.repo.get_all_local_branches()? {
            let local_branch_oid = match branch.get_oid()? {
                Some(branch_oid) => branch_oid,
                None => continue,
            };
            if !self.dag.set_contains(&commit_set, local_branch_oid)? {
                continue;
            }
            let local_branch_name = branch.get_name()?;

            let (remote_name, remote_branch_name) = match branch.get_upstream_branch()? {
                Some(upstream_branch) => {
                    let remote_branch_name = upstream_branch.get_name()?;
                    let remote_name = branch.get_push_remote_name()?;
                    let remote_branch_name = match &remote_name {
                        None => remote_branch_name,
                        Some(remote_name) => {
                            match remote_branch_name.strip_prefix(&format!("{remote_name}/")) {
                                Some(remote_branch_name) => remote_branch_name,
                                None => remote_branch_name,
                            }
                        }
                    };
                    (remote_name, Some(remote_branch_name.to_owned()))
                }
                None => (None, None),
            };

            let submit_status = match &remote_branch_name {
                None => SubmitStatus::Unsubmitted,
                Some(remote_branch_name) => {
                    match remote_branch_name_to_pr_info.get(remote_branch_name) {
                        None => SubmitStatus::Unsubmitted,
                        Some(pr_info) => {
                            let SerializedNonZeroOid(remote_branch_oid) = pr_info.head_ref_oid;
                            if local_branch_oid == remote_branch_oid {
                                SubmitStatus::UpToDate
                            } else {
                                SubmitStatus::NeedsUpdate
                            }
                        }
                    }
                }
            };
            result.insert(
                local_branch_oid,
                CommitStatus {
                    submit_status,
                    local_branch_name: Some(local_branch_name.to_owned()),
                    remote_name,
                    remote_branch_name,
                },
            );
        }

        for commit_oid in self.dag.commit_set_to_vec(&commit_set)? {
            if !result.contains_key(&commit_oid) {
                result.insert(
                    commit_oid,
                    CommitStatus {
                        submit_status: SubmitStatus::Unsubmitted,
                        remote_name: None,
                        local_branch_name: None,
                        remote_branch_name: None,
                    },
                );
            }
        }

        Ok(Ok(result))
    }

    #[instrument]
    fn create(
        &mut self,
        commits: HashMap<NonZeroOid, CommitStatus>,
        options: &SubmitOptions,
    ) -> EyreExitOr<HashMap<NonZeroOid, CreateStatus>> {
        let effects = self.effects;
        let commits_to_create = commits
            .iter()
            .filter_map(
                |(commit_oid, commit_status)| match commit_status.submit_status {
                    SubmitStatus::Local
                    | SubmitStatus::Unknown
                    | SubmitStatus::NeedsUpdate
                    | SubmitStatus::UpToDate => None,
                    SubmitStatus::Unsubmitted => Some((commit_oid, commit_status)),
                },
            )
            .collect_vec();

        let github_username = try_exit_code!(self.query_github_username()?);
        let mut commits_to_push = HashMap::new();
        for (commit_oid, commit_status) in commits_to_create {
            let commit = self.repo.find_commit_or_fail(*commit_oid)?;

            let local_branch_name = match &commit_status.local_branch_name {
                Some(local_branch_name) => local_branch_name.clone(),
                None => {
                    let summary = commit.get_summary()?;
                    let summary = String::from_utf8_lossy(&summary);
                    let summary_slug: String = summary
                        .chars()
                        .map(|c| if c.is_alphanumeric() { c } else { '-' })
                        .flat_map(|c| c.to_lowercase())
                        .collect();
                    let new_branch_name_base = format!("{github_username}/{summary_slug}");
                    let mut new_branch_name = new_branch_name_base.clone();
                    for i in 2.. {
                        if i > 6 {
                            writeln!(
                                effects.get_output_stream(),
                                "Could not generate fresh branch name for commit: {}",
                                effects
                                    .get_glyphs()
                                    .render(commit.friendly_describe(effects.get_glyphs())?)?,
                            )?;
                            return Ok(Err(ExitCode(1)));
                        }
                        match self.repo.find_branch(&new_branch_name, BranchType::Local)? {
                            Some(_) => {
                                new_branch_name = format!("{new_branch_name_base}-{i}");
                            }
                            None => break,
                        }
                    }
                    self.repo.create_branch(&new_branch_name, &commit, false)?;
                    new_branch_name
                }
            };

            commits_to_push.insert(
                *commit_oid,
                CommitStatus {
                    submit_status: SubmitStatus::Unsubmitted,
                    local_branch_name: Some(local_branch_name),
                    remote_name: None,
                    remote_branch_name: None,
                },
            );
        }
        dbg!(&commits_to_push);

        let references_snapshot = self.repo.get_references_snapshot()?;
        let mut branch_forge = BranchForge {
            effects: self.effects,
            git_run_info: self.git_run_info,
            dag: self.dag,
            repo: self.repo,
            event_log_db: self.event_log_db,
            references_snapshot: &references_snapshot,
        };
        branch_forge.create(commits_to_push, options)
    }

    #[instrument]
    fn update(
        &mut self,
        commits: HashMap<NonZeroOid, CommitStatus>,
        options: &SubmitOptions,
    ) -> EyreExitOr<()> {
        let SubmitOptions {
            create: _,
            draft: _,
            execution_strategy: _,
            num_jobs: _,
        } = options;

        let now = SystemTime::now();
        let event_tx_id = self
            .event_log_db
            .make_transaction_id(now, "GithubForge::update")?;
        let remotes_to_branches = commits
            .into_iter()
            .filter_map(
                |(_commit_oid, commit_status)| match &commit_status.remote_name {
                    Some(remote_name) => Some((remote_name.clone(), commit_status)),
                    None => None,
                },
            )
            .into_group_map();
        for (remote_name, commit_statuses) in remotes_to_branches
            .into_iter()
            .sorted_by_key(|(remote_name, _commit_status)| remote_name.clone())
        {
            let mut args = vec![
                "push".to_string(),
                "--force-with-lease".to_string(),
                remote_name,
            ];
            args.extend(
                commit_statuses
                    .into_iter()
                    .filter_map(|commit_status| commit_status.local_branch_name)
                    .sorted(),
            );
            try_exit_code!(self
                .git_run_info
                .run(self.effects, Some(event_tx_id), &args)?);
        }

        Ok(Ok(()))
    }
}

impl GithubForge<'_> {
    #[instrument]
    fn run_gh(&self, args: &[&str]) -> EyreExitOr<Vec<u8>> {
        let exe = "gh";
        let exe_invocation = format!("{exe} {}", args.join(" "));
        debug!(?exe_invocation, "Invoking gh");

        let child = Command::new("gh")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("Invoking `gh` command-line executable")?;
        let output = child
            .wait_with_output()
            .context("Waiting for `gh` invocation")?;
        if !output.status.success() {
            writeln!(
                self.effects.get_output_stream(),
                "Call to `{exe_invocation}` failed",
            )?;
            writeln!(self.effects.get_output_stream(), "Stdout:")?;
            writeln!(
                self.effects.get_output_stream(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            )?;
            writeln!(self.effects.get_output_stream(), "Stderr:")?;
            writeln!(
                self.effects.get_output_stream(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            )?;
            return Ok(Err(ExitCode::try_from(output.status)?));
        }
        Ok(Ok(output.stdout))
    }

    #[instrument]
    fn query_github_username(&self) -> EyreExitOr<String> {
        let username = try_exit_code!(self.run_gh(&["api", "user", "--jq", ".login"])?);
        let username = String::from_utf8(username)?;
        let username = username.trim().to_owned();
        Ok(Ok(username))
    }
}
