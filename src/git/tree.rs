use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eyre::Context;
use itertools::Itertools;
use tracing::{instrument, warn};

use super::oid::make_non_zero_oid;
use super::{MaybeZeroOid, NonZeroOid, Repo};

/// A tree object. Contains a mapping from name to OID.
#[derive(Debug)]
pub struct Tree<'repo> {
    pub(super) inner: git2::Tree<'repo>,
}

impl Tree<'_> {
    /// Get the object ID for this tree.
    pub fn get_oid(&self) -> NonZeroOid {
        make_non_zero_oid(self.inner.id())
    }

    /// Determine whether this tree is empty (i.e. contains no entries).
    ///
    /// This doesn't happen in typical practice, since the index can't represent
    /// empty directories (trees). However, this can happen when operating on
    /// trees directly.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Get the OID for the entry with the given path.
    ///
    /// Note that the path isn't just restricted to entries of the current tree,
    /// i.e. you can use slashes in the provided path.
    pub fn get_oid_for_path(&self, path: &Path) -> eyre::Result<Option<MaybeZeroOid>> {
        match self.inner.get_path(path) {
            Ok(entry) => Ok(Some(entry.id().into())),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }
}

/// Add the provided entries into the tree.
///
/// If the provided `Tree` is `None`, then this function adds the entries to the
/// empty tree.
///
/// The paths for the provided entries can contain slashes.
///
/// If the value for an entry is `None`, then that element in the tree is
/// removed. If a directory ever becomes empty, then it's removed from its
/// parent directory.
///
/// If a path for a given entry is already present in the provided tree, then
/// that entry is overwritten.
///
/// If a path refers to intermediate directories that don't exist in the
/// provided tree, then those intermediate directories are created.
#[instrument]
pub fn hydrate_tree(
    repo: &Repo,
    tree: Option<&Tree>,
    entries: HashMap<PathBuf, Option<(NonZeroOid, i32)>>,
) -> eyre::Result<NonZeroOid> {
    let (file_entries, dir_entries) = {
        let mut file_entries: HashMap<PathBuf, Option<(NonZeroOid, i32)>> = HashMap::new();
        let mut dir_entries: HashMap<PathBuf, HashMap<PathBuf, Option<(NonZeroOid, i32)>>> =
            HashMap::new();
        for (path, value) in entries {
            match path.components().collect_vec().as_slice() {
                [] => {
                    warn!(?tree, ?value, "Empty path when hydrating tree");
                }
                [file_name] => {
                    file_entries.insert(file_name.into(), value);
                }
                components => {
                    let first: PathBuf = [components[0]].iter().collect();
                    let rest: PathBuf = components[1..].iter().collect();
                    dir_entries.entry(first).or_default().insert(rest, value);
                }
            }
        }
        (file_entries, dir_entries)
    };

    let tree = tree.map(|tree| &tree.inner);
    let mut builder = repo
        .inner
        .treebuilder(tree)
        .wrap_err_with(|| "Instantiating tree builder")?;
    for (file_name, file_value) in file_entries {
        match file_value {
            Some((oid, file_mode)) => {
                builder
                    .insert(&file_name, oid.inner, file_mode)
                    .wrap_err_with(|| {
                        format!(
                            "Inserting file {:?} with OID: {:?}, file mode: {:?}",
                            &file_name, oid, file_mode
                        )
                    })?;
            }
            None => {
                remove_entry_if_exists(&mut builder, &file_name)
                    .wrap_err_with(|| format!("Removing deleted file: {:?}", &file_name))?;
            }
        }
    }

    for (dir_name, dir_value) in dir_entries {
        let existing_dir_entry: Option<Tree> = match builder.get(&dir_name)? {
            Some(existing_dir_entry)
                if !existing_dir_entry.id().is_zero()
                    && existing_dir_entry.kind() == Some(git2::ObjectType::Tree) =>
            {
                repo.find_tree(make_non_zero_oid(existing_dir_entry.id()))?
            }
            _ => None,
        };
        let new_entry_oid = hydrate_tree(repo, existing_dir_entry.as_ref(), dir_value)?;

        let new_entry_tree = repo
            .find_tree(new_entry_oid)?
            .ok_or_else(|| eyre::eyre!("Could not find just-hydrated tree: {:?}", new_entry_oid))?;
        if new_entry_tree.is_empty() {
            remove_entry_if_exists(&mut builder, &dir_name)
                .wrap_err_with(|| format!("Removing empty directory: {:?}", &dir_name))?;
        } else {
            builder
                .insert(&dir_name, new_entry_oid.inner, git2::FileMode::Tree.into())
                .wrap_err_with(|| {
                    format!(
                        "Inserting directory {:?} with OID: {:?}",
                        &dir_name, new_entry_oid
                    )
                })?;
        }
    }

    let tree_oid = builder.write().wrap_err_with(|| "Building tree")?;
    Ok(make_non_zero_oid(tree_oid))
}

/// `libgit2` raises an error if the entry isn't present, but that's often not
/// an error condition here. We may be referring to a created or deleted path,
/// which wouldn't exist in one of the pre-/post-patch trees.
fn remove_entry_if_exists(builder: &mut git2::TreeBuilder, name: &Path) -> eyre::Result<()> {
    if builder.get(&name)?.is_some() {
        builder.remove(&name)?;
    }
    Ok(())
}

/// Filter the entries in the provided tree by only keeping the provided paths.
///
/// If a provided path does not appear in the tree at all, then it's ignored.
#[instrument]
pub fn dehydrate_tree(repo: &Repo, tree: &Tree, paths: &[&Path]) -> eyre::Result<NonZeroOid> {
    let entries: HashMap<PathBuf, Option<(NonZeroOid, i32)>> = paths
        .iter()
        .map(|path| -> eyre::Result<(PathBuf, _)> {
            let key = path.to_path_buf();
            match tree.inner.get_path(path) {
                Ok(tree_entry) => {
                    let value = Some((make_non_zero_oid(tree_entry.id()), tree_entry.filemode()));
                    Ok((key, value))
                }
                Err(err) if err.code() == git2::ErrorCode::NotFound => Ok((key, None)),
                Err(err) => Err(err.into()),
            }
        })
        .try_collect()?;

    hydrate_tree(repo, None, entries)
}

#[cfg(test)]
mod tests {
    use std::convert::TryInto;

    use super::*;

    use crate::testing::make_git;

    fn dump_tree_entries(tree: &Tree) -> String {
        tree.inner
            .iter()
            .map(|entry| format!("{:?} {:?}\n", entry.name().unwrap(), entry.id()))
            .collect()
    }

    #[test]
    fn test_hydrate_tree() -> eyre::Result<()> {
        let git = make_git()?;

        git.init_repo()?;

        git.write_file("foo", "foo")?;
        git.write_file("bar/bar", "bar")?;
        git.write_file("bar/baz", "qux")?;
        git.write_file("xyzzy", "xyzzy")?;
        git.run(&["add", "."])?;
        git.run(&["commit", "-m", "commit"])?;

        let repo = git.get_repo()?;
        let head_oid = repo.get_head_info()?.oid.unwrap();
        let head_commit = repo.find_commit_or_fail(head_oid)?;
        let head_tree = head_commit.get_tree()?;

        insta::assert_snapshot!(dump_tree_entries(&head_tree), @r###"
        "bar" 778e23a1e80b1feb10e00b15b29a33315929c5b5
        "foo.txt" 19102815663d23f8b75a47e7a01965dcdc96468c
        "initial.txt" 63af22885f8665a312ba8b83db722134f1f8290d
        "xyzzy.txt" 7c465afc533f95ff7d2c91e18921f94aac8292fc
        "###);

        {
            let hydrated_tree = {
                let hydrated_tree_oid = hydrate_tree(&repo, Some(&head_tree), {
                    let mut result = HashMap::new();
                    result.insert(
                        PathBuf::from("foo-copy.txt"),
                        Some((
                            head_tree
                                .get_oid_for_path(&PathBuf::from("foo.txt"))?
                                .unwrap()
                                .try_into()?,
                            0o100644,
                        )),
                    );
                    result.insert(PathBuf::from("foo.txt"), None);
                    result
                })?;
                repo.find_tree(hydrated_tree_oid)?.unwrap()
            };
            insta::assert_snapshot!(dump_tree_entries(&hydrated_tree), @r###"
            "bar" 778e23a1e80b1feb10e00b15b29a33315929c5b5
            "foo-copy.txt" 19102815663d23f8b75a47e7a01965dcdc96468c
            "initial.txt" 63af22885f8665a312ba8b83db722134f1f8290d
            "xyzzy.txt" 7c465afc533f95ff7d2c91e18921f94aac8292fc
            "###);
        }

        {
            let hydrated_tree = {
                let hydrated_tree_oid = hydrate_tree(&repo, Some(&head_tree), {
                    let mut result = HashMap::new();
                    result.insert(PathBuf::from("bar/bar.txt"), None);
                    result
                })?;
                repo.find_tree(hydrated_tree_oid)?.unwrap()
            };
            insta::assert_snapshot!(dump_tree_entries(&hydrated_tree), @r###"
            "bar" 08ee88e1c53fbd01ab76f136a4f2c9d759b981d0
            "foo.txt" 19102815663d23f8b75a47e7a01965dcdc96468c
            "initial.txt" 63af22885f8665a312ba8b83db722134f1f8290d
            "xyzzy.txt" 7c465afc533f95ff7d2c91e18921f94aac8292fc
            "###);
        }

        {
            let hydrated_tree = {
                let hydrated_tree_oid = hydrate_tree(&repo, Some(&head_tree), {
                    let mut result = HashMap::new();
                    result.insert(PathBuf::from("bar/bar.txt"), None);
                    result.insert(PathBuf::from("bar/baz.txt"), None);
                    result
                })?;
                repo.find_tree(hydrated_tree_oid)?.unwrap()
            };
            insta::assert_snapshot!(dump_tree_entries(&hydrated_tree), @r###"
            "foo.txt" 19102815663d23f8b75a47e7a01965dcdc96468c
            "initial.txt" 63af22885f8665a312ba8b83db722134f1f8290d
            "xyzzy.txt" 7c465afc533f95ff7d2c91e18921f94aac8292fc
            "###);
        }

        {
            let dehydrated_tree_oid = dehydrate_tree(
                &repo,
                &head_tree,
                &[Path::new("bar/baz.txt"), Path::new("foo.txt")],
            )?;
            let dehydrated_tree = repo.find_tree(dehydrated_tree_oid)?.unwrap();
            insta::assert_snapshot!(dump_tree_entries(&dehydrated_tree), @r###"
            "bar" 08ee88e1c53fbd01ab76f136a4f2c9d759b981d0
            "foo.txt" 19102815663d23f8b75a47e7a01965dcdc96468c
            "###);
        }

        Ok(())
    }
}
