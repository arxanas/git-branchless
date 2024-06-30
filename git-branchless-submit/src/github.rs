//! GitHub backend for submitting patch stacks.

use std::collections::HashMap;
use std::env;
use std::fmt::{Debug, Write};
use std::hash::Hash;

use indexmap::IndexMap;
use itertools::Itertools;
use lib::core::config::get_main_branch_name;
use lib::core::dag::CommitSet;
use lib::core::dag::Dag;
use lib::core::effects::Effects;
use lib::core::effects::OperationType;
use lib::core::eventlog::EventLogDb;
use lib::core::formatting::{Effect, StyledString};
use lib::core::repo_ext::RepoExt;
use lib::core::repo_ext::RepoReferencesSnapshot;
use lib::git::CategorizedReferenceName;
use lib::git::GitErrorCode;
use lib::git::GitRunInfo;
use lib::git::RepoError;
use lib::git::{BranchType, ConfigRead};
use lib::git::{NonZeroOid, Repo};
use lib::try_exit_code;
use lib::util::ExitCode;
use lib::util::EyreExitOr;

use tracing::debug;
use tracing::instrument;
use tracing::warn;

use crate::branch_forge::BranchForge;
use crate::SubmitStatus;
use crate::{CommitStatus, CreateStatus, Forge, SubmitOptions};

/// Testing environment variable. When this is set, the executable will use the
/// mock Github implementation. This should be set to the path of an existing
/// repository that represents the remote/Github.
pub const MOCK_REMOTE_REPO_PATH_ENV_KEY: &str = "BRANCHLESS_SUBMIT_GITHUB_MOCK_REMOTE_REPO_PATH";

fn commit_summary_slug(summary: &str) -> String {
    let summary_slug: String = summary
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .flat_map(|c| c.to_lowercase())
        .dedup_by(|lhs, rhs| {
            // Deduplicate adjacent hyphens.
            *lhs == '-' && *rhs == '-'
        })
        .collect();
    let summary_slug = summary_slug.trim_matches('-');
    if summary_slug.is_empty() {
        "to-review".to_string()
    } else {
        summary_slug.to_owned()
    }
}

fn singleton<K: Debug + Eq + Hash, V: Clone>(
    map: &HashMap<K, V>,
    key: K,
    f: impl Fn(V) -> V,
) -> HashMap<K, V> {
    let mut result = HashMap::new();
    match map.get(&key) {
        Some(value) => {
            result.insert(key, f(value.clone()));
        }
        None => {
            warn!(?key, "No match for key in map");
        }
    }
    result
}

/// Get the name of the remote repository to push to in the course of creating
/// pull requests.
///
/// NOTE: The `gh` command-line utility might infer the push remote if only one
/// remote is available. This function only returns a remote if it is explicitly
/// set by the user using `gh repo set-default`.`
pub fn github_push_remote(repo: &Repo) -> eyre::Result<Option<String>> {
    let config = repo.get_readonly_config()?;
    for remote_name in repo.get_all_remote_names()? {
        // This is set by `gh repo set-default`. Note that `gh` can
        // sometimes infer which repo to push to without invoking
        // `gh repo set-default` explicitly. We could probably check
        // the remote URL to see if it's associated with
        // `github.com`, but we would still need `gh` to be
        // installed on the system in that case. The presence of
        // this value means that `gh` was actively used.
        //
        // Possible values seem to be `base` and `other`. See:
        // https://github.com/search?q=repo%3Acli%2Fcli%20gh-resolved&type=code
        let gh_resolved: Option<String> =
            config.get(format!("remote.{remote_name}.gh-resolved"))?;
        if gh_resolved.as_deref() == Some("base") {
            return Ok(Some(remote_name));
        }
    }
    Ok(None)
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
    pub client: Box<dyn client::GithubClient>,
}

impl Forge for GithubForge<'_> {
    #[instrument]
    fn query_status(
        &mut self,
        commit_set: CommitSet,
    ) -> EyreExitOr<HashMap<NonZeroOid, CommitStatus>> {
        let effects = self.effects;
        let pull_request_infos =
            try_exit_code!(self.client.query_repo_pull_request_infos(effects)?);
        let references_snapshot = self.repo.get_references_snapshot()?;

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
            let remote_name = branch.get_push_remote_name()?;
            let remote_branch_name = branch.get_upstream_branch_name_without_push_remote_name()?;

            let submit_status = match remote_branch_name
                .as_ref()
                .and_then(|remote_branch_name| pull_request_infos.get(remote_branch_name))
            {
                None => SubmitStatus::Unsubmitted,
                Some(pull_request_info) => {
                    let updated_pull_request_info = try_exit_code!(self
                        .make_updated_pull_request_info(
                            effects,
                            &references_snapshot,
                            &pull_request_infos,
                            local_branch_oid
                        )?);
                    debug!(
                        ?pull_request_info,
                        ?updated_pull_request_info,
                        "Comparing pull request info"
                    );
                    if updated_pull_request_info
                        .fields_to_update(pull_request_info)
                        .is_empty()
                    {
                        SubmitStatus::UpToDate
                    } else {
                        SubmitStatus::NeedsUpdate
                    }
                }
            };
            result.insert(
                local_branch_oid,
                CommitStatus {
                    submit_status,
                    remote_name,
                    local_commit_name: Some(local_branch_name.to_owned()),
                    remote_commit_name: remote_branch_name,
                },
            );
        }

        for commit_oid in self.dag.commit_set_to_vec(&commit_set)? {
            result.entry(commit_oid).or_insert(CommitStatus {
                submit_status: SubmitStatus::Unsubmitted,
                remote_name: None,
                local_commit_name: None,
                remote_commit_name: None,
            });
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
        let commit_oids = self.dag.sort(&commits.keys().copied().collect())?;

        let references_snapshot = self.repo.get_references_snapshot()?;
        let mut branch_forge = BranchForge {
            effects,
            git_run_info: self.git_run_info,
            dag: self.dag,
            repo: self.repo,
            event_log_db: self.event_log_db,
            references_snapshot: &references_snapshot,
        };
        let push_remote_name = match github_push_remote(self.repo)? {
            Some(remote_name) => remote_name,
            None => match self.repo.get_default_push_remote()? {
                Some(remote_name) => remote_name,
                None => {
                    writeln!(
                        effects.get_output_stream(),
                        "No default push repository configured. To configure, run: {}",
                        effects.get_glyphs().render(StyledString::styled(
                            "gh repo set-default <repo>",
                            Effect::Bold,
                        ))?
                    )?;
                    return Ok(Err(ExitCode(1)));
                }
            },
        };
        let github_username = try_exit_code!(self.client.query_github_username(effects)?);

        // Generate branches for all the commits to create.
        let commits_to_create = commit_oids
            .into_iter()
            .map(|commit_oid| (commit_oid, commits.get(&commit_oid).unwrap()))
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
        let mut created_branches = HashMap::new();
        for (commit_oid, commit_status) in commits_to_create.iter().copied() {
            let commit = self.repo.find_commit_or_fail(commit_oid)?;

            let local_branch_name = match &commit_status.local_commit_name {
                Some(local_branch_name) => local_branch_name.clone(),
                None => {
                    let summary = commit.get_summary()?;
                    let summary = String::from_utf8_lossy(&summary);
                    let summary_slug = commit_summary_slug(&summary);
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
                    match self.repo.create_branch(&new_branch_name, &commit, false) {
                        Ok(_branch) => {}
                        Err(RepoError::CreateBranch { source, name: _ })
                            if source.code() == GitErrorCode::Exists => {}
                        Err(err) => return Err(err.into()),
                    };
                    new_branch_name
                }
            };

            let created_branch = try_exit_code!(branch_forge.create(
                singleton(&commits, commit_oid, |commit_status| CommitStatus {
                    local_commit_name: Some(local_branch_name.clone()),
                    ..commit_status.clone()
                }),
                options
            )?);
            for (commit_oid, create_status) in created_branch.iter() {
                match create_status {
                    CreateStatus::Created { .. } => {}
                    CreateStatus::Skipped | CreateStatus::Err => {
                        // FIXME: surface the inner branch forge error somehow?
                        writeln!(
                            effects.get_output_stream(),
                            "Could not create branch for commit: {}",
                            effects.get_glyphs().render(
                                self.repo.friendly_describe_commit_from_oid(
                                    effects.get_glyphs(),
                                    *commit_oid
                                )?,
                            )?
                        )?;
                        return Ok(Err(ExitCode(1)));
                    }
                }
            }
            created_branches.extend(created_branch.into_iter());
        }

        let commit_statuses: HashMap<NonZeroOid, CommitStatus> = commits_to_create
            .iter()
            .copied()
            .map(|(commit_oid, commit_status)| {
                let commit_status = match created_branches.get(&commit_oid) {
                    Some(CreateStatus::Created {
                        final_commit_oid: _,
                        local_commit_name,
                    }) => CommitStatus {
                        // To be updated below:
                        submit_status: SubmitStatus::NeedsUpdate,
                        remote_name: Some(push_remote_name.clone()),
                        local_commit_name: Some(local_commit_name.clone()),
                        // Expecting this to be the same as the local branch name (for now):
                        remote_commit_name: Some(local_commit_name.clone()),
                    },

                    Some(
                        CreateStatus::Skipped  | CreateStatus::Err ,
                    ) => {
                        warn!(?commits_to_create, ?created_branches, ?commit_oid, ?commit_status, "commit failed to be created");
                        eyre::bail!("BUG: should have been handled in previous call to branch_forge.create: {commit_oid:?} has status {commit_status:?}");
                    }
                    None => commit_status.clone(),
                };
                Ok((commit_oid, commit_status))
            })
            .try_collect()?;

        // Create the pull requests only after creating all the branches because
        // we rely on the presence of a branch on each commit in the stack to
        // know that it should be included/linked in the pull request body.
        // FIXME: is this actually necessary?
        for (commit_oid, _) in commits_to_create {
            let local_branch_name = match commit_statuses.get(&commit_oid) {
                Some(CommitStatus {
                    local_commit_name: Some(local_commit_name),
                    ..
                }) => local_commit_name,
                Some(CommitStatus {
                    local_commit_name: None,
                    ..
                })
                | None => {
                    writeln!(
                        effects.get_output_stream(),
                        "Could not find local branch name for commit: {}",
                        effects.get_glyphs().render(
                            self.repo
                                .find_commit_or_fail(commit_oid)?
                                .friendly_describe(effects.get_glyphs())?
                        )?
                    )?;
                    return Ok(Err(ExitCode(1)));
                }
            };

            let commit = self.repo.find_commit_or_fail(commit_oid)?;
            let title = String::from_utf8_lossy(&commit.get_summary()?).into_owned();
            let body = String::from_utf8_lossy(&commit.get_message_pretty()).into_owned();
            try_exit_code!(self.client.create_pull_request(
                effects,
                client::CreatePullRequestArgs {
                    head_ref_oid: commit_oid,
                    head_ref_name: local_branch_name.clone(),
                    title,
                    body,
                },
                options
            )?);
        }

        try_exit_code!(self.update(commit_statuses, options)?);

        Ok(Ok(created_branches))
    }

    #[instrument]
    fn update(
        &mut self,
        commit_statuses: HashMap<NonZeroOid, CommitStatus>,
        options: &SubmitOptions,
    ) -> EyreExitOr<()> {
        let effects = self.effects;
        let SubmitOptions {
            create: _,
            draft: _,
            execution_strategy: _,
            num_jobs: _,
            message: _,
        } = options;

        let pull_request_infos =
            try_exit_code!(self.client.query_repo_pull_request_infos(effects)?);
        let references_snapshot = self.repo.get_references_snapshot()?;
        let mut branch_forge = BranchForge {
            effects,
            git_run_info: self.git_run_info,
            dag: self.dag,
            repo: self.repo,
            event_log_db: self.event_log_db,
            references_snapshot: &references_snapshot,
        };

        let commit_set: CommitSet = commit_statuses.keys().copied().collect();
        let commit_oids = self.dag.sort(&commit_set)?;
        {
            let (effects, progress) = effects.start_operation(OperationType::UpdateCommits);
            progress.notify_progress(0, commit_oids.len());
            for commit_oid in commit_oids {
                let commit_status = match commit_statuses.get(&commit_oid) {
                    Some(commit_status) => commit_status,
                    None => {
                        warn!(
                            ?commit_oid,
                            ?commit_statuses,
                            "Commit not found in commit statuses"
                        );
                        continue;
                    }
                };
                let remote_branch_name = match &commit_status.remote_commit_name {
                    Some(remote_branch_name) => remote_branch_name,
                    None => {
                        warn!(
                            ?commit_oid,
                            ?commit_statuses,
                            "Commit does not have remote branch name"
                        );
                        continue;
                    }
                };
                let pull_request_info = match pull_request_infos.get(remote_branch_name) {
                    Some(pull_request_info) => pull_request_info,
                    None => {
                        warn!(
                            ?commit_oid,
                            ?commit_statuses,
                            "Commit does not have pull request"
                        );
                        continue;
                    }
                };

                let updated_pull_request_info = try_exit_code!(self
                    .make_updated_pull_request_info(
                        &effects,
                        &references_snapshot,
                        &pull_request_infos,
                        commit_oid
                    )?);
                let updated_fields = {
                    let fields = updated_pull_request_info.fields_to_update(pull_request_info);
                    if fields.is_empty() {
                        "none (this should not happen)".to_owned()
                    } else {
                        fields.join(", ")
                    }
                };
                let client::UpdatePullRequestArgs {
                    head_ref_oid: _, // Updated by `branch_forge.update`.
                    base_ref_name,
                    title,
                    body,
                } = updated_pull_request_info;
                writeln!(
                    effects.get_output_stream(),
                    "Updating pull request ({updated_fields}) for commit {}",
                    effects.get_glyphs().render(
                        self.repo
                            .find_commit_or_fail(commit_oid)?
                            .friendly_describe(effects.get_glyphs())?
                    )?
                )?;

                // Make sure to update the branch and metadata at the same time,
                // rather than all the branches at first. Otherwise, when
                // reordering commits, GitHub may close one of the pull requests
                // as it seems to have all the commits of its parent (or
                // something like that).

                // Push branch:
                try_exit_code!(
                    branch_forge.update(singleton(&commit_statuses, commit_oid, |x| x), options)?
                );

                // Update metdata:
                try_exit_code!(self.client.update_pull_request(
                    &effects,
                    pull_request_info.number,
                    client::UpdatePullRequestArgs {
                        head_ref_oid: commit_oid,
                        base_ref_name,
                        title,
                        body,
                    },
                    options
                )?);
                progress.notify_progress_inc(1);
            }
        }

        Ok(Ok(()))
    }
}

impl GithubForge<'_> {
    /// Construct a real or mock GitHub client according to the environment.
    pub fn client(git_run_info: GitRunInfo) -> Box<dyn client::GithubClient> {
        match env::var(MOCK_REMOTE_REPO_PATH_ENV_KEY) {
            Ok(path) => Box::new(client::MockGithubClient {
                remote_repo_path: path.into(),
            }),
            Err(_) => {
                let GitRunInfo {
                    path_to_git: _,
                    working_directory,
                    env,
                } = git_run_info;
                let gh_run_info = GitRunInfo {
                    path_to_git: "gh".into(),
                    working_directory: working_directory.clone(),
                    env: env.clone(),
                };
                Box::new(client::RealGithubClient { gh_run_info })
            }
        }
    }

    #[instrument]
    fn make_updated_pull_request_info(
        &self,
        effects: &Effects,
        references_snapshot: &RepoReferencesSnapshot,
        pull_request_infos: &HashMap<String, client::PullRequestInfo>,
        commit_oid: NonZeroOid,
    ) -> EyreExitOr<client::UpdatePullRequestArgs> {
        let mut stack_index = None;
        let mut stack_pull_request_infos: IndexMap<NonZeroOid, &client::PullRequestInfo> =
            Default::default();

        // Ensure we iterate over the stack in topological order so that the
        // stack indexes are correct.
        let stack_commit_oids = self
            .dag
            .sort(&self.dag.query_stack_commits(CommitSet::from(commit_oid))?)?;
        let get_pull_request_info =
            |commit_oid: NonZeroOid| -> eyre::Result<Option<&client::PullRequestInfo>> {
                let commit = self.repo.find_commit_or_fail(commit_oid)?; // for debug output

                debug!(?commit, "Checking commit for pull request info");
                let stack_branch_names =
                    match references_snapshot.branch_oid_to_names.get(&commit_oid) {
                        Some(stack_branch_names) => stack_branch_names,
                        None => {
                            debug!(?commit, "Commit has no associated branches");
                            return Ok(None);
                        }
                    };

                // The commit should have at most one associated branch with a pull
                // request.
                for stack_branch_name in stack_branch_names.iter().sorted() {
                    let stack_local_branch = match self.repo.find_branch(
                        &CategorizedReferenceName::new(stack_branch_name).render_suffix(),
                        BranchType::Local,
                    )? {
                        Some(stack_local_branch) => stack_local_branch,
                        None => {
                            debug!(
                                ?commit,
                                ?stack_branch_name,
                                "Skipping branch with no local branch"
                            );
                            continue;
                        }
                    };

                    let stack_remote_branch_name = match stack_local_branch
                        .get_upstream_branch_name_without_push_remote_name()?
                    {
                        Some(stack_remote_branch_name) => stack_remote_branch_name,
                        None => {
                            debug!(
                                ?commit,
                                ?stack_local_branch,
                                "Skipping local branch with no remote branch"
                            );
                            continue;
                        }
                    };

                    let pull_request_info = match pull_request_infos.get(&stack_remote_branch_name)
                    {
                        Some(pull_request_info) => pull_request_info,
                        None => {
                            debug!(
                                ?commit,
                                ?stack_local_branch,
                                ?stack_remote_branch_name,
                                "Skipping remote branch with no pull request info"
                            );
                            continue;
                        }
                    };

                    debug!(
                        ?commit,
                        ?pull_request_info,
                        "Found pull request info for commit"
                    );
                    return Ok(Some(pull_request_info));
                }

                debug!(
                    ?commit,
                    "Commit has no branches with associated pull request info"
                );
                Ok(None)
            };
        for stack_commit_oid in stack_commit_oids {
            let pull_request_info = match get_pull_request_info(stack_commit_oid)? {
                Some(info) => info,
                None => continue,
            };
            stack_pull_request_infos.insert(stack_commit_oid, pull_request_info);
            if stack_commit_oid == commit_oid {
                stack_index = Some(stack_pull_request_infos.len());
            }
        }

        let stack_size = stack_pull_request_infos.len();
        if stack_size == 0 {
            warn!(
                ?commit_oid,
                ?stack_pull_request_infos,
                "No pull requests in stack for commit"
            );
        }
        let stack_index = match stack_index {
            Some(stack_index) => stack_index.to_string(),
            None => {
                warn!(
                    ?commit_oid,
                    ?stack_pull_request_infos,
                    "Could not determine index in stack for commit"
                );
                "?".to_string()
            }
        };

        let stack_list = {
            let mut result = String::new();
            for stack_pull_request_info in stack_pull_request_infos.values() {
                // Github will render a lone pull request URL as a title and
                // open/closed status.
                writeln!(result, "* {}", stack_pull_request_info.url)?;
            }
            result
        };

        let commit = self.repo.find_commit_or_fail(commit_oid)?;
        let commit_summary = commit.get_summary()?;
        let commit_summary = String::from_utf8_lossy(&commit_summary).into_owned();
        let title = format!("[{stack_index}/{stack_size}] {commit_summary}");
        let commit_message = commit.get_message_pretty();
        let commit_message = String::from_utf8_lossy(&commit_message);
        let body = format!(
            "\
**Stack:**

{stack_list}

---

{commit_message}
"
        );

        let stack_ancestor_oids = {
            let main_branch_oid = CommitSet::from(references_snapshot.main_branch_oid);
            let stack_ancestor_oids = self
                .dag
                .query_only(CommitSet::from(commit_oid), main_branch_oid)?
                .difference(&CommitSet::from(commit_oid));
            self.dag.commit_set_to_vec(&stack_ancestor_oids)?
        };
        let nearest_ancestor_with_pull_request_info = {
            let mut result = None;
            for stack_ancestor_oid in stack_ancestor_oids.into_iter().rev() {
                if let Some(info) = get_pull_request_info(stack_ancestor_oid)? {
                    result = Some(info);
                    break;
                }
            }
            result
        };
        let base_ref_name = match nearest_ancestor_with_pull_request_info {
            Some(info) => info.head_ref_name.clone(),
            None => get_main_branch_name(self.repo)?,
        };

        Ok(Ok(client::UpdatePullRequestArgs {
            head_ref_oid: commit_oid,
            base_ref_name,
            title,
            body,
        }))
    }
}

mod client {
    use std::collections::{BTreeMap, HashMap};
    use std::fmt::{Debug, Write};
    use std::fs::{self, File};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::Arc;

    use eyre::Context;
    use itertools::Itertools;
    use lib::core::dag::Dag;
    use lib::core::effects::{Effects, OperationType};
    use lib::core::eventlog::{EventLogDb, EventReplayer};
    use lib::core::formatting::Glyphs;
    use lib::core::repo_ext::RepoExt;
    use lib::git::{GitRunInfo, NonZeroOid, Repo, SerializedNonZeroOid};
    use lib::try_exit_code;
    use lib::util::{ExitCode, EyreExitOr};
    use serde::{Deserialize, Serialize};
    use tempfile::NamedTempFile;
    use tracing::{debug, instrument};

    use crate::SubmitOptions;

    #[derive(Clone, Debug, Deserialize, Serialize)]
    pub struct PullRequestInfo {
        #[serde(rename = "number")]
        pub number: usize,
        #[serde(rename = "url")]
        pub url: String,
        #[serde(rename = "headRefName")]
        pub head_ref_name: String,
        #[serde(rename = "headRefOid")]
        pub head_ref_oid: SerializedNonZeroOid,
        #[serde(rename = "baseRefName")]
        pub base_ref_name: String,
        #[serde(rename = "closed")]
        pub closed: bool,
        #[serde(rename = "isDraft")]
        pub is_draft: bool,
        #[serde(rename = "title")]
        pub title: String,
        #[serde(rename = "body")]
        pub body: String,
    }

    #[derive(Debug)]
    pub struct CreatePullRequestArgs {
        pub head_ref_oid: NonZeroOid,
        pub head_ref_name: String,
        pub title: String,
        pub body: String,
    }

    #[derive(Debug, Eq, PartialEq)]
    pub struct UpdatePullRequestArgs {
        pub head_ref_oid: NonZeroOid,
        pub base_ref_name: String,
        pub title: String,
        pub body: String,
    }

    impl UpdatePullRequestArgs {
        pub fn fields_to_update(&self, pull_request_info: &PullRequestInfo) -> Vec<&'static str> {
            let PullRequestInfo {
                number: _,
                url: _,
                head_ref_name: _,
                head_ref_oid: SerializedNonZeroOid(old_head_ref_oid),
                base_ref_name: old_base_ref_name,
                closed: _,
                is_draft: _,
                title: old_title,
                body: old_body,
            } = pull_request_info;
            let Self {
                head_ref_oid: new_head_ref_oid,
                base_ref_name: new_base_ref_name,
                title: new_title,
                body: new_body,
            } = self;

            let mut updated_fields = Vec::new();
            if old_head_ref_oid != new_head_ref_oid {
                updated_fields.push("commit");
            }
            if old_base_ref_name != new_base_ref_name {
                updated_fields.push("base branch");
            }
            if old_title != new_title {
                updated_fields.push("title");
            }
            if old_body != new_body {
                updated_fields.push("body");
            }
            updated_fields
        }
    }

    pub trait GithubClient: Debug {
        /// Get the username of the currently-logged-in user.
        fn query_github_username(&self, effects: &Effects) -> EyreExitOr<String>;

        /// Get the details of all pull requests for the currently-logged-in user in
        /// the current repository. The resulting map is keyed by remote branch
        /// name.
        fn query_repo_pull_request_infos(
            &self,
            effects: &Effects,
        ) -> EyreExitOr<HashMap<String, PullRequestInfo>>;

        fn create_pull_request(
            &self,
            effects: &Effects,
            args: CreatePullRequestArgs,
            submit_options: &super::SubmitOptions,
        ) -> EyreExitOr<String>;

        fn update_pull_request(
            &self,
            effects: &Effects,
            number: usize,
            args: UpdatePullRequestArgs,
            submit_options: &super::SubmitOptions,
        ) -> EyreExitOr<()>;
    }

    #[derive(Debug)]
    pub struct RealGithubClient {
        #[allow(dead_code)] // FIXME: destructure and use in `run_gh`?
        pub gh_run_info: GitRunInfo,
    }

    impl RealGithubClient {
        #[instrument]
        fn run_gh(&self, effects: &Effects, args: &[&str]) -> EyreExitOr<Vec<u8>> {
            let exe = "gh";
            let exe_invocation = format!("{exe} {}", args.join(" "));
            debug!(?exe_invocation, "Invoking gh");
            let (effects, progress) =
                effects.start_operation(OperationType::RunTests(Arc::new(exe_invocation.clone())));
            let _progress = progress;

            let child = Command::new("gh")
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("Invoking `gh` command-line executable")?;
            let output = child
                .wait_with_output()
                .context("Waiting for `gh` invocation")?;
            if !output.status.success() {
                writeln!(
                    effects.get_output_stream(),
                    "Call to `{exe_invocation}` failed",
                )?;
                writeln!(effects.get_output_stream(), "Stdout:")?;
                writeln!(
                    effects.get_output_stream(),
                    "{}",
                    String::from_utf8_lossy(&output.stdout)
                )?;
                writeln!(effects.get_output_stream(), "Stderr:")?;
                writeln!(
                    effects.get_output_stream(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                )?;
                return Ok(Err(ExitCode::try_from(output.status)?));
            }
            Ok(Ok(output.stdout))
        }

        #[instrument]
        fn write_body_file(&self, body: &str) -> eyre::Result<NamedTempFile> {
            use std::io::Write;
            let mut body_file = NamedTempFile::new()?;
            body_file.write_all(body.as_bytes())?;
            body_file.flush()?;
            Ok(body_file)
        }
    }

    impl GithubClient for RealGithubClient {
        /// Get the username of the currently-logged-in user.
        #[instrument]
        fn query_github_username(&self, effects: &Effects) -> EyreExitOr<String> {
            let username =
                try_exit_code!(self.run_gh(effects, &["api", "user", "--jq", ".login"])?);
            let username = String::from_utf8(username)?;
            let username = username.trim().to_owned();
            Ok(Ok(username))
        }

        /// Get the details of all pull requests for the currently-logged-in user in
        /// the current repository. The resulting map is keyed by remote branch
        /// name.
        #[instrument]
        fn query_repo_pull_request_infos(
            &self,
            effects: &Effects,
        ) -> EyreExitOr<HashMap<String, PullRequestInfo>> {
            let output = try_exit_code!(self.run_gh(
                effects,
                &[
                    "pr",
                    "list",
                    "--author",
                    "@me",
                    "--json",
                    "number,url,headRefName,headRefOid,baseRefName,closed,isDraft,title,body",
                ]
            )?);
            let pull_request_infos: Vec<PullRequestInfo> =
                serde_json::from_slice(&output).wrap_err("Deserializing output from gh pr list")?;
            let pull_request_infos = pull_request_infos
                .into_iter()
                .map(|item| (item.head_ref_name.clone(), item))
                .collect();
            Ok(Ok(pull_request_infos))
        }

        #[instrument]
        fn create_pull_request(
            &self,
            effects: &Effects,
            args: CreatePullRequestArgs,
            submit_options: &SubmitOptions,
        ) -> EyreExitOr<String> {
            let CreatePullRequestArgs {
                head_ref_oid: _,
                head_ref_name,
                title,
                body,
            } = args;
            let body_file = self.write_body_file(&body)?;
            let mut args = vec![
                "pr",
                "create",
                "--head",
                &head_ref_name,
                "--title",
                &title,
                "--body-file",
                body_file.path().to_str().unwrap(),
            ];

            let SubmitOptions {
                create: _,
                draft,
                execution_strategy: _,
                num_jobs: _,
                message: _,
            } = submit_options;
            if *draft {
                args.push("--draft");
            }

            let stdout = try_exit_code!(self.run_gh(effects, &args)?);
            let pull_request_url = match std::str::from_utf8(&stdout) {
                Ok(url) => url,
                Err(err) => {
                    writeln!(
                        effects.get_output_stream(),
                        "Could not parse output from `gh pr create` as UTF-8: {err}",
                    )?;
                    return Ok(Err(ExitCode(1)));
                }
            };
            let pull_request_url = pull_request_url.trim();
            Ok(Ok(pull_request_url.to_owned()))
        }

        fn update_pull_request(
            &self,
            effects: &Effects,
            number: usize,
            args: UpdatePullRequestArgs,
            _submit_options: &super::SubmitOptions,
        ) -> EyreExitOr<()> {
            let UpdatePullRequestArgs {
                head_ref_oid: _, // branch should have been pushed by caller
                base_ref_name,
                title,
                body,
            } = args;
            let body_file = self.write_body_file(&body)?;
            try_exit_code!(self.run_gh(
                effects,
                &[
                    "pr",
                    "edit",
                    &number.to_string(),
                    "--base",
                    &base_ref_name,
                    "--title",
                    &title,
                    "--body-file",
                    (body_file.path().to_str().unwrap()),
                ],
            )?);
            Ok(Ok(()))
        }
    }

    /// The mock state on disk, representing the remote Github repository and
    /// server.
    #[derive(Debug, Default, Deserialize, Serialize)]
    pub struct MockState {
        /// The next index to assign a newly-created pull request.
        pub pull_request_index: usize,

        /// Information about all pull requests open for the repository. Sorted
        /// for determinism when dumping state for testing.
        pub pull_requests: BTreeMap<String, PullRequestInfo>,
    }

    impl MockState {
        fn load(path: &Path) -> eyre::Result<Self> {
            let file = match File::open(path) {
                Ok(file) => file,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Default::default());
                }
                Err(err) => return Err(err).wrap_err("Opening mock GitHub client state file"),
            };
            let state = serde_json::from_reader(file)?;
            Ok(state)
        }

        fn restore_invariants(&mut self, remote_repo: &Repo) -> eyre::Result<()> {
            let effects = Effects::new_suppress_for_test(Glyphs::text());
            let conn = remote_repo.get_db_conn()?;
            let event_log_db = EventLogDb::new(&conn)?;
            let event_replayer =
                EventReplayer::from_event_log_db(&effects, remote_repo, &event_log_db)?;
            let event_cursor = event_replayer.make_default_cursor();
            let references_snapshot = remote_repo.get_references_snapshot()?;
            let dag = Dag::open_and_sync(
                &effects,
                remote_repo,
                &event_replayer,
                event_cursor,
                &references_snapshot,
            )?;

            let branches: HashMap<String, NonZeroOid> = remote_repo
                .get_all_local_branches()?
                .into_iter()
                .map(|branch| -> eyre::Result<_> {
                    let branch_name = branch.get_name()?.to_owned();
                    let branch_oid = branch.get_oid()?.unwrap();
                    Ok((branch_name, branch_oid))
                })
                .try_collect()?;
            for (_, pull_request_info) in self.pull_requests.iter_mut() {
                let base_ref_name = &pull_request_info.base_ref_name;
                let base_branch_oid = match branches.get(base_ref_name) {
                    Some(oid) => *oid,
                    None => {
                        eyre::bail!("Could not find base branch {base_ref_name:?} for pull request: {pull_request_info:?}");
                    }
                };
                let SerializedNonZeroOid(head_ref_oid) = pull_request_info.head_ref_oid;
                if dag.query_is_ancestor(head_ref_oid, base_branch_oid)? {
                    pull_request_info.closed = true;
                }
            }
            Ok(())
        }

        fn save(&self, path: &Path) -> eyre::Result<()> {
            let state = serde_json::to_string_pretty(self)?;
            fs::write(path, state)?;
            Ok(())
        }
    }

    /// A mock client representing the remote Github repository and server.
    #[derive(Debug)]
    pub struct MockGithubClient {
        /// The path to the remote repository on disk.
        pub remote_repo_path: PathBuf,
    }

    impl GithubClient for MockGithubClient {
        fn query_github_username(&self, _effects: &Effects) -> EyreExitOr<String> {
            Ok(Ok(Self::username().to_owned()))
        }

        fn query_repo_pull_request_infos(
            &self,
            _effects: &Effects,
        ) -> EyreExitOr<HashMap<String, PullRequestInfo>> {
            let pull_requests_infos = self.with_state_mut(|state| {
                let pull_request_infos = state
                    .pull_requests
                    .values()
                    .cloned()
                    .map(|pull_request_info| {
                        (pull_request_info.head_ref_name.clone(), pull_request_info)
                    })
                    .collect();
                Ok(pull_request_infos)
            })?;
            Ok(Ok(pull_requests_infos))
        }

        fn create_pull_request(
            &self,
            _effects: &Effects,
            args: CreatePullRequestArgs,
            submit_options: &super::SubmitOptions,
        ) -> EyreExitOr<String> {
            let url = self.with_state_mut(|state| {
                state.pull_request_index += 1;
                let CreatePullRequestArgs {
                    head_ref_oid,
                    head_ref_name,
                    title,
                    body,
                } = args;
                let SubmitOptions {
                    create,
                    draft,
                    execution_strategy: _,
                    num_jobs: _,
                    message: _,
                } = submit_options;
                assert!(create);
                let url = format!(
                    "https://example.com/{}/{}/pulls/{}",
                    Self::username(),
                    Self::repo_name(),
                    state.pull_request_index
                );
                let pull_request_info = PullRequestInfo {
                    number: state.pull_request_index,
                    url: url.clone(),
                    head_ref_name: head_ref_name.clone(),
                    head_ref_oid: SerializedNonZeroOid(head_ref_oid),
                    base_ref_name: Self::main_branch().to_owned(),
                    closed: false,
                    is_draft: *draft,
                    title,
                    body,
                };
                state.pull_requests.insert(head_ref_name, pull_request_info);
                Ok(url)
            })?;
            Ok(Ok(url))
        }

        fn update_pull_request(
            &self,
            _effects: &Effects,
            number: usize,
            args: UpdatePullRequestArgs,
            _submit_options: &super::SubmitOptions,
        ) -> EyreExitOr<()> {
            self.with_state_mut(|state| -> eyre::Result<()> {
                let UpdatePullRequestArgs {
                    head_ref_oid,
                    base_ref_name,
                    title,
                    body,
                } = args;
                let pull_request_info = match state
                    .pull_requests
                    .values_mut()
                    .find(|pull_request_info| pull_request_info.number == number)
                {
                    Some(pull_request_info) => pull_request_info,
                    None => {
                        eyre::bail!("Could not find pull request with number {number}");
                    }
                };
                pull_request_info.head_ref_oid = SerializedNonZeroOid(head_ref_oid);
                pull_request_info.base_ref_name = base_ref_name;
                pull_request_info.title = title;
                pull_request_info.body = body;
                Ok(())
            })?;
            Ok(Ok(()))
        }
    }

    impl MockGithubClient {
        fn username() -> &'static str {
            "mock-github-username"
        }

        fn repo_name() -> &'static str {
            "mock-github-repo"
        }

        fn main_branch() -> &'static str {
            "master"
        }

        /// Get the path on disk where the mock state is stored.
        pub fn state_path(&self) -> PathBuf {
            self.remote_repo_path.join("mock-github-client-state.json")
        }

        /// Load the mock state from disk, run the given function, and then save
        /// the state back to disk. Github-specific pull request invariants are
        /// restored before and after running the function.
        pub fn with_state_mut<T>(
            &self,
            f: impl FnOnce(&mut MockState) -> eyre::Result<T>,
        ) -> eyre::Result<T> {
            let repo = Repo::from_dir(&self.remote_repo_path)?;
            let state_path = self.state_path();
            let mut state = MockState::load(&state_path)?;
            state.restore_invariants(&repo)?;
            let result = f(&mut state)?;
            state.restore_invariants(&repo)?;
            state.save(&state_path)?;
            Ok(result)
        }
    }
}

/// Testing utilities.
pub mod testing {
    pub use super::client::MockGithubClient;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_commit_summary_slug() {
        assert_eq!(commit_summary_slug("hello: foo bar"), "hello-foo-bar");
        assert_eq!(
            commit_summary_slug("category(topic): `foo` bar!"),
            "category-topic-foo-bar"
        );
        assert_eq!(commit_summary_slug("foo_~_bar"), "foo-bar");
        assert_eq!(commit_summary_slug("!!!"), "to-review")
    }
}
