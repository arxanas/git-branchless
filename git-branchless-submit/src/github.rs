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
use lib::git::GitRunInfo;
use lib::git::SerializedNonZeroOid;
use lib::git::{NonZeroOid, Repo};
use lib::try_exit_code;
use lib::util::ExitCode;
use lib::util::EyreExitOr;

use serde::Deserialize;

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
    fn query_status(
        &mut self,
        commit_set: CommitSet,
    ) -> EyreExitOr<HashMap<NonZeroOid, CommitStatus>> {
        let exe = "gh";
        let args = [
            "pr",
            "list",
            "--author",
            "@me",
            "--json",
            "headRefName,headRefOid",
        ];
        let exe_invocation = format!("{exe} {}", args.join(" "));
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

        let pr_infos: Vec<PullRequestInfo> = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("Deserializing output from {exe_invocation}"))?;
        let remote_branch_name_to_pr_info: HashMap<String, PullRequestInfo> = pr_infos
            .into_iter()
            .map(|item| (item.head_ref_name.clone(), item))
            .collect();

        let mut result = HashMap::new();
        for branch in self.repo.get_all_local_branches()? {
            let upstream_branch = match branch.get_upstream_branch()? {
                Some(upstream_branch) => upstream_branch,
                None => continue,
            };
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

            let pr_info = match remote_branch_name_to_pr_info.get(remote_branch_name) {
                None => continue,
                Some(pr_info) => pr_info,
            };

            let local_branch_oid = match branch.get_oid()? {
                Some(branch_oid) => branch_oid,
                None => continue,
            };
            if !self.dag.set_contains(&commit_set, local_branch_oid)? {
                continue;
            }

            let SerializedNonZeroOid(remote_branch_oid) = pr_info.head_ref_oid;
            let submit_status = if local_branch_oid == remote_branch_oid {
                SubmitStatus::UpToDate
            } else {
                SubmitStatus::NeedsUpdate
            };

            let local_branch_name = branch.get_name()?;
            result.insert(
                local_branch_oid,
                CommitStatus {
                    submit_status,
                    remote_name,
                    local_branch_name: Some(local_branch_name.to_owned()),
                    remote_branch_name: Some(remote_branch_name.to_owned()),
                },
            );
        }

        Ok(Ok(result))
    }

    fn create(
        &mut self,
        _commits: HashMap<NonZeroOid, CommitStatus>,
        _options: &SubmitOptions,
    ) -> EyreExitOr<HashMap<NonZeroOid, CreateStatus>> {
        todo!()
    }

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
