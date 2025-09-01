//! Helper functions on [`Repo`].

use std::collections::{HashMap, HashSet};

use color_eyre::Help;
use eyre::Context;
use tracing::instrument;

use crate::git::{
    Branch, BranchType, CategorizedReferenceName, ConfigRead, NonZeroOid, ReferenceName, Repo,
};

use super::config::get_main_branch_name;

/// A snapshot of all the positions of references we care about in the repository.
#[derive(Debug)]
pub struct RepoReferencesSnapshot {
    /// The location of the `HEAD` reference. This may be `None` if `HEAD` is unborn.
    pub head_oid: Option<NonZeroOid>,

    /// The location of the main branch.
    pub main_branch_oid: NonZeroOid,

    /// A mapping from commit OID to the branches which point to that commit.
    pub branch_oid_to_names: HashMap<NonZeroOid, HashSet<ReferenceName>>,
}

/// Helper functions on [`Repo`].
pub trait RepoExt {
    /// Get the `Branch` for the main branch for the repository.
    fn get_main_branch(&self) -> eyre::Result<Branch<'_>>;

    /// Get the OID corresponding to the main branch.
    fn get_main_branch_oid(&self) -> eyre::Result<NonZeroOid>;

    /// Get a mapping from OID to the names of branches which point to that OID.
    ///
    /// The returned branch names include the `refs/heads/` prefix, so it must
    /// be stripped if desired.
    fn get_branch_oid_to_names(&self) -> eyre::Result<HashMap<NonZeroOid, HashSet<ReferenceName>>>;

    /// Get the positions of references in the repository.
    fn get_references_snapshot(&self) -> eyre::Result<RepoReferencesSnapshot>;

    /// Get the default remote to push to for new branches in this repository.
    fn get_default_push_remote(&self) -> eyre::Result<Option<String>>;
}

impl RepoExt for Repo {
    fn get_main_branch(&self) -> eyre::Result<Branch<'_>> {
        let main_branch_name = get_main_branch_name(self)?;
        match self.find_branch(&main_branch_name, BranchType::Local)? {
            Some(branch) => Ok(branch),
            None => {
                let suggestion = format!(
                    r"
The main branch {:?} could not be found in your repository
at path: {:?}.
These branches exist: {:?}
Either create it, or update the main branch setting by running:

    git branchless init --main-branch <branch>

Note that remote main branches are no longer supported as of v0.6.0. See
https://github.com/arxanas/git-branchless/discussions/595 for more details.",
                    get_main_branch_name(self)?,
                    self.get_path(),
                    self.get_all_local_branches()?
                        .into_iter()
                        .map(|branch| {
                            branch
                                .into_reference()
                                .get_name()
                                .map(|s| format!("{s:?}"))
                                .wrap_err("converting branch to reference")
                        })
                        .collect::<eyre::Result<Vec<String>>>()?,
                );
                Err(eyre::eyre!("Could not find repository main branch")
                    .with_suggestion(|| suggestion))
            }
        }
    }

    #[instrument]
    fn get_main_branch_oid(&self) -> eyre::Result<NonZeroOid> {
        let main_branch = self.get_main_branch()?;
        let main_branch_oid = main_branch.get_oid()?;
        match main_branch_oid {
            Some(main_branch_oid) => Ok(main_branch_oid),
            None => eyre::bail!(
                "Could not find commit pointed to by main branch: {:?}",
                main_branch.get_name()?,
            ),
        }
    }

    #[instrument]
    fn get_branch_oid_to_names(&self) -> eyre::Result<HashMap<NonZeroOid, HashSet<ReferenceName>>> {
        let mut result: HashMap<NonZeroOid, HashSet<ReferenceName>> = HashMap::new();
        for branch in self.get_all_local_branches()? {
            let reference = branch.into_reference();
            let reference_name = reference.get_name()?;
            let reference_info = self.resolve_reference(&reference)?;
            if let Some(reference_oid) = reference_info.oid {
                result
                    .entry(reference_oid)
                    .or_default()
                    .insert(reference_name);
            }
        }

        Ok(result)
    }

    fn get_references_snapshot(&self) -> eyre::Result<RepoReferencesSnapshot> {
        let head_oid = self.get_head_info()?.oid;
        let main_branch_oid = self.get_main_branch_oid()?;
        let branch_oid_to_names = self.get_branch_oid_to_names()?;

        Ok(RepoReferencesSnapshot {
            head_oid,
            main_branch_oid,
            branch_oid_to_names,
        })
    }

    fn get_default_push_remote(&self) -> eyre::Result<Option<String>> {
        let main_branch_name = self.get_main_branch()?.get_reference_name()?;
        match CategorizedReferenceName::new(&main_branch_name) {
            name @ CategorizedReferenceName::LocalBranch { .. } => {
                if let Some(main_branch) =
                    self.find_branch(&name.render_suffix(), BranchType::Local)?
                {
                    if let Some(remote_name) = main_branch.get_push_remote_name()? {
                        return Ok(Some(remote_name));
                    }
                }
            }

            name @ CategorizedReferenceName::RemoteBranch { .. } => {
                let name = name.render_suffix();
                if let Some((remote_name, _reference_name)) = name.split_once('/') {
                    return Ok(Some(remote_name.to_owned()));
                }
            }

            CategorizedReferenceName::OtherRef { .. } => {
                // Do nothing.
            }
        }

        let push_default_remote_opt = self.get_readonly_config()?.get("remote.pushDefault")?;
        Ok(push_default_remote_opt)
    }
}
