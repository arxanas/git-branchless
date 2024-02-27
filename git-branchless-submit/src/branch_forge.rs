use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;
use std::time::SystemTime;

use itertools::Itertools;
use lib::core::config::get_main_branch_name;
use lib::core::dag::{CommitSet, Dag};
use lib::core::effects::{Effects, OperationType};
use lib::core::eventlog::EventLogDb;
use lib::core::repo_ext::{RepoExt, RepoReferencesSnapshot};
use lib::git::{
    Branch, BranchType, CategorizedReferenceName, GitRunInfo, NonZeroOid, ReferenceName, Repo,
};
use lib::try_exit_code;
use lib::util::{ExitCode, EyreExitOr};
use tracing::warn;

use crate::{CommitStatus, CreateStatus, Forge, SubmitOptions, SubmitStatus};

#[derive(Debug)]
pub struct BranchForge<'a> {
    pub effects: &'a Effects,
    pub git_run_info: &'a GitRunInfo,
    pub repo: &'a Repo,
    pub dag: &'a Dag,
    pub event_log_db: &'a EventLogDb<'a>,
    pub references_snapshot: &'a RepoReferencesSnapshot,
}

impl Forge for BranchForge<'_> {
    fn query_status(
        &mut self,
        commit_set: CommitSet,
    ) -> EyreExitOr<HashMap<NonZeroOid, CommitStatus>> {
        struct BranchInfo<'a> {
            branch: Branch<'a>,
            branch_name: String,
            remote_name: Option<String>,
        }
        let main_branch_name = get_main_branch_name(self.repo)?;
        let branch_infos: HashMap<ReferenceName, BranchInfo> = {
            let branch_reference_names = self
                .dag
                .commit_set_to_vec(&commit_set)?
                .into_iter()
                .flat_map(|commit_oid| {
                    match self
                        .references_snapshot
                        .branch_oid_to_names
                        .get(&commit_oid)
                    {
                        Some(names) => names.iter().cloned().collect(),
                        None => Vec::new(),
                    }
                });

            let mut branch_infos = HashMap::new();
            for branch_reference_name in branch_reference_names {
                let branch_name = match CategorizedReferenceName::new(&branch_reference_name) {
                    name @ CategorizedReferenceName::LocalBranch { .. } => name.render_suffix(),
                    CategorizedReferenceName::RemoteBranch { .. }
                    | CategorizedReferenceName::OtherRef { .. } => continue,
                };
                if branch_name == main_branch_name {
                    continue;
                }
                let branch = self
                    .repo
                    .find_branch(&branch_name, BranchType::Local)?
                    .ok_or_else(|| eyre::eyre!("Could not look up branch {branch_name:?}"))?;

                let remote_name = branch.get_push_remote_name()?;
                let branch_info = BranchInfo {
                    branch,
                    branch_name,
                    remote_name,
                };
                branch_infos.insert(branch_reference_name, branch_info);
            }
            branch_infos
        };

        // Fetch latest branches so that we know which commits are out-of-date
        // and need to be pushed.
        let event_tx_id = self
            .event_log_db
            .make_transaction_id(SystemTime::now(), "fetch remotes")?;
        let remote_to_branches: BTreeMap<&String, Vec<&Branch>> = branch_infos
            .values()
            .flat_map(|branch_info| {
                let BranchInfo {
                    branch,
                    branch_name: _,
                    remote_name,
                } = branch_info;
                remote_name
                    .as_ref()
                    .map(|remote_name| (remote_name, branch))
            })
            .into_group_map()
            .into_iter()
            .collect();
        // Make sure not to call `git fetch` with no remotes, as that will fetch
        // something by default.
        for (remote_name, branches) in remote_to_branches.iter() {
            let remote_args = {
                let mut result = vec!["fetch".to_owned()];
                result.push((*remote_name).clone());
                let branch_reference_names: BTreeSet<ReferenceName> = branches
                    .iter()
                    .map(|branch| branch.get_reference_name())
                    .try_collect()?;
                for branch_reference_name in branch_reference_names {
                    result.push(branch_reference_name.as_str().to_owned());
                }
                result
            };
            match self
                .git_run_info
                .run(self.effects, Some(event_tx_id), &remote_args)?
            {
                Ok(()) => {}
                Err(exit_code) => {
                    writeln!(
                        self.effects.get_output_stream(),
                        "Failed to fetch from remote: {}",
                        remote_name
                    )?;
                    return Ok(Err(exit_code));
                }
            }
        }

        // Determine status of each commit/branch.
        let mut commit_statuses = HashMap::new();
        for (commit_oid, branches) in &self.references_snapshot.branch_oid_to_names {
            let branch_infos = branches
                .iter()
                .sorted()
                .flat_map(|branch_reference_name| branch_infos.get(branch_reference_name))
                .collect_vec();

            let commit_status = match branch_infos.as_slice() {
                [] => CommitStatus {
                    submit_status: SubmitStatus::Local,
                    remote_name: None,
                    local_commit_name: None,
                    remote_commit_name: None,
                },

                [BranchInfo {
                    branch,
                    branch_name,
                    remote_name,
                }] => match branch.get_upstream_branch()? {
                    None => CommitStatus {
                        submit_status: SubmitStatus::Unsubmitted,
                        remote_name: None,
                        local_commit_name: Some(branch_name.clone()),
                        remote_commit_name: None,
                    },

                    Some(upstream_branch) => CommitStatus {
                        submit_status: if branch.get_oid()? == upstream_branch.get_oid()? {
                            SubmitStatus::UpToDate
                        } else {
                            SubmitStatus::NeedsUpdate
                        },
                        remote_name: remote_name.clone(),
                        local_commit_name: Some(branch_name.clone()),
                        remote_commit_name: Some(upstream_branch.get_name()?.to_owned()),
                    },
                },

                _branch_infos => CommitStatus {
                    submit_status: SubmitStatus::Unknown,
                    remote_name: None,
                    local_commit_name: None,
                    remote_commit_name: None,
                },
            };
            commit_statuses.insert(*commit_oid, commit_status);
        }

        Ok(Ok(commit_statuses))
    }

    fn create(
        &mut self,
        commits: HashMap<NonZeroOid, CommitStatus>,
        _options: &SubmitOptions,
    ) -> EyreExitOr<HashMap<NonZeroOid, CreateStatus>> {
        let unsubmitted_branch_names = commits
            .values()
            .filter_map(|commit_status| {
                let CommitStatus {
                    submit_status: _,
                    remote_name: _,
                    local_commit_name,
                    remote_commit_name: _,
                } = commit_status;
                local_commit_name.clone()
            })
            .sorted()
            .collect_vec();

        let push_remote: String = match self.repo.get_default_push_remote()? {
            Some(push_remote) => push_remote,
            None => {
                writeln!(
                    self.effects.get_output_stream(),
                    "\
No upstream repository was associated with {} and no value was
specified for `remote.pushDefault`, so cannot push these branches: {}
Configure a value with: git config remote.pushDefault <remote>
These remotes are available: {}",
                    CategorizedReferenceName::new(
                        &self.repo.get_main_branch()?.get_reference_name()?,
                    )
                    .friendly_describe(),
                    unsubmitted_branch_names.join(", "),
                    self.repo.get_all_remote_names()?.join(", "),
                )?;
                return Ok(Err(ExitCode(1)));
            }
        };

        if unsubmitted_branch_names.is_empty() {
            Ok(Ok(Default::default()))
        } else {
            // This will fail if somebody else created the branch on the remote and we don't
            // know about it.
            let mut args = vec!["push", "--set-upstream", &push_remote];
            args.extend(unsubmitted_branch_names.iter().map(|s| s.as_str()));
            let event_tx_id = self
                .event_log_db
                .make_transaction_id(SystemTime::now(), "submit unsubmitted commits")?;
            let (effects, progress) = self.effects.start_operation(OperationType::PushCommits);
            let _effects = effects;
            progress.notify_progress(0, unsubmitted_branch_names.len());
            try_exit_code!(self
                .git_run_info
                .run(self.effects, Some(event_tx_id), &args)?);
            Ok(Ok(commits
                .into_iter()
                .filter_map(|(commit_oid, commit_status)| {
                    commit_status.local_commit_name.map(|local_commit_name| {
                        (
                            commit_oid,
                            CreateStatus::Created {
                                final_commit_oid: commit_oid,
                                local_commit_name,
                            },
                        )
                    })
                })
                .collect()))
        }
    }

    fn update(
        &mut self,
        commits: HashMap<NonZeroOid, CommitStatus>,
        _options: &SubmitOptions,
    ) -> EyreExitOr<()> {
        let branches_by_remote: BTreeMap<String, BTreeSet<String>> = commits
            .into_values()
            .flat_map(|commit_status| match commit_status {
                CommitStatus {
                    submit_status: _,
                    remote_name: Some(remote_name),
                    local_commit_name: Some(local_commit_name),
                    remote_commit_name: _,
                } => Some((remote_name, local_commit_name)),
                commit_status => {
                    warn!(
                        ?commit_status,
"Commit was requested to be updated, but it did not have the requisite information."
                    );
                    None
                }
            })
            .into_group_map()
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().collect::<BTreeSet<_>>()))
            .collect();

        let now = SystemTime::now();
        let event_tx_id = self.event_log_db.make_transaction_id(now, "submit")?;
        let (effects, progress) = self.effects.start_operation(OperationType::PushCommits);
        let total_num_branches = branches_by_remote
            .values()
            .map(|branch_names| branch_names.len())
            .sum();
        progress.notify_progress(0, total_num_branches);
        for (remote_name, branch_names) in branches_by_remote {
            let mut args = vec!["push", "--force-with-lease", &remote_name];
            args.extend(branch_names.iter().map(|s| s.as_str()));
            match self.git_run_info.run(&effects, Some(event_tx_id), &args)? {
                Ok(()) => {}
                Err(exit_code) => {
                    writeln!(
                        effects.get_output_stream(),
                        "Failed to push branches: {}",
                        branch_names.into_iter().join(", ")
                    )?;
                    return Ok(Err(exit_code));
                }
            }
            progress.notify_progress_inc(branch_names.len());
        }

        Ok(Ok(()))
    }
}
