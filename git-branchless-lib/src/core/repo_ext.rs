//! Helper functions on [`Repo`].

use std::collections::{HashMap, HashSet};

use color_eyre::Help;
use tracing::instrument;

use crate::git::{NonZeroOid, Reference, ReferenceName, Repo};

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
    /// Get the `Reference` for the main branch for the repository.
    fn get_main_branch_reference(&self) -> eyre::Result<Reference>;

    /// Get the OID corresponding to the main branch.
    fn get_main_branch_oid(&self) -> eyre::Result<NonZeroOid>;

    /// Get a mapping from OID to the names of branches which point to that OID.
    ///
    /// The returned branch names include the `refs/heads/` prefix, so it must
    /// be stripped if desired.
    fn get_branch_oid_to_names(&self) -> eyre::Result<HashMap<NonZeroOid, HashSet<ReferenceName>>>;

    /// Get the positions of references in the repository.
    fn get_references_snapshot(&self) -> eyre::Result<RepoReferencesSnapshot>;
}

impl RepoExt for Repo {
    fn get_main_branch_reference(&self) -> eyre::Result<Reference> {
        let main_branch_name = get_main_branch_name(self)?;
        match self.find_branch(&main_branch_name, git2::BranchType::Local)? {
            Some(branch) => match branch.get_upstream_branch()? {
                Some(upstream_branch) => Ok(upstream_branch.into_reference()),
                None => Ok(branch.into_reference()),
            },
            None => match self.find_branch(&main_branch_name, git2::BranchType::Remote)? {
                Some(branch) => Ok(branch.into_reference()),
                None => {
                    let suggestion = format!(
                        r"
The main branch {:?} could not be found in your repository
at path: {:?}.
These branches exist: {:?}
Either create it, or update the main branch setting by running:

    git config branchless.core.mainBranch <branch>
",
                        get_main_branch_name(self)?,
                        self.get_path(),
                        self.get_all_local_branches()?
                            .into_iter()
                            .map(|branch| {
                                branch
                                    .into_reference()
                                    .get_name()
                                    .map(|s| format!("{:?}", s))
                            })
                            .collect::<eyre::Result<Vec<String>>>()?,
                    );
                    Err(eyre::eyre!("Could not find repository main branch")
                        .with_suggestion(|| suggestion))
                }
            },
        }
    }

    #[instrument]
    fn get_main_branch_oid(&self) -> eyre::Result<NonZeroOid> {
        let main_branch_reference = self.get_main_branch_reference()?;
        let commit = main_branch_reference.peel_to_commit()?;
        match commit {
            Some(commit) => Ok(commit.get_oid()),
            None => eyre::bail!(
                "Could not find commit pointed to by main branch: {:?}",
                main_branch_reference.get_name()?
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
                    .or_insert_with(HashSet::new)
                    .insert(reference_name);
            }
        }

        // The main branch may be a remote branch, in which case it won't be
        // returned in the iteration above.
        let main_branch_name = self.get_main_branch_reference()?.get_name()?;
        let main_branch_oid = self.get_main_branch_oid()?;
        result
            .entry(main_branch_oid)
            .or_insert_with(HashSet::new)
            .insert(main_branch_name);

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
}
