//! Hooks used to have Git call back into `git-branchless` for various functionality.

use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::ffi::{OsStr, OsString};
use std::fmt::Write;
use std::fs::File;
use std::io::{stdin, BufRead, BufReader, Read, Write as WriteIo};
use std::path::Path;
use std::time::SystemTime;

use console::style;
use eyre::Context;
use itertools::Itertools;
use tempfile::NamedTempFile;
use tracing::instrument;

use crate::core::config::{get_restack_warn_abandoned, RESTACK_WARN_ABANDONED_CONFIG_KEY};
use crate::core::eventlog::{Event, EventLogDb, EventReplayer, EventTransactionId};
use crate::core::formatting::{printable_styled_string, Pluralize};
use crate::core::graph::{make_graph, BranchOids, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::git::{
    CategorizedReferenceName, GitRunInfo, MaybeZeroOid, NonZeroOid, ReferenceTarget, Repo,
};
use crate::tui::Effects;

use super::{find_abandoned_children, move_branches};

#[instrument(skip(stream))]
fn read_rewritten_list_entries(
    stream: &mut impl Read,
) -> eyre::Result<Vec<(NonZeroOid, MaybeZeroOid)>> {
    let mut rewritten_oids = Vec::new();
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        match *line.split(' ').collect::<Vec<_>>().as_slice() {
            [old_commit_oid, new_commit_oid, ..] => {
                let old_commit_oid: NonZeroOid = old_commit_oid.parse()?;
                let new_commit_oid: MaybeZeroOid = new_commit_oid.parse()?;
                rewritten_oids.push((old_commit_oid, new_commit_oid));
            }
            _ => eyre::bail!("Invalid rewrite line: {:?}", &line),
        }
    }
    Ok(rewritten_oids)
}

#[instrument]
fn write_rewritten_list(
    rewritten_list_path: &Path,
    rewritten_oids: &[(NonZeroOid, MaybeZeroOid)],
) -> eyre::Result<()> {
    let mut tempfile = NamedTempFile::new()?;
    let file = tempfile.as_file_mut();
    for (old_commit_oid, new_commit_oid) in rewritten_oids {
        writeln!(file, "{} {}", old_commit_oid, new_commit_oid)?;
    }
    tempfile.persist(rewritten_list_path).wrap_err_with(|| {
        format!(
            "Moving new rewritten-list into place at: {:?}",
            rewritten_list_path
        )
    })?;
    Ok(())
}

#[instrument]
fn add_rewritten_list_entries(
    rewritten_list_path: &Path,
    entries: &[(NonZeroOid, MaybeZeroOid)],
) -> eyre::Result<()> {
    let current_entries = match File::open(rewritten_list_path) {
        Ok(mut rewritten_list_file) => read_rewritten_list_entries(&mut rewritten_list_file)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(err) => return Err(err.into()),
    };

    let mut entries_to_add: HashMap<NonZeroOid, MaybeZeroOid> = entries.iter().copied().collect();
    let mut new_entries = Vec::new();
    for (old_commit_oid, new_commit_oid) in current_entries {
        let new_entry = match entries_to_add.remove(&old_commit_oid) {
            Some(new_commit_oid) => (old_commit_oid, new_commit_oid),
            None => (old_commit_oid, new_commit_oid),
        };
        new_entries.push(new_entry);
    }
    new_entries.extend(entries_to_add.into_iter());

    write_rewritten_list(rewritten_list_path, new_entries.as_slice())?;
    Ok(())
}

/// Handle Git's `post-rewrite` hook.
///
/// See the man-page for `githooks(5)`.
#[instrument]
pub fn hook_post_rewrite(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    rewrite_type: &str,
) -> eyre::Result<()> {
    let now = SystemTime::now();
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();

    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let mut event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "hook-post-rewrite")?;

    let (rewritten_oids, events) = {
        let rewritten_oids = read_rewritten_list_entries(&mut stdin().lock())?;
        let events = rewritten_oids
            .iter()
            .copied()
            .map(|(old_commit_oid, new_commit_oid)| Event::RewriteEvent {
                timestamp,
                event_tx_id,
                old_commit_oid: old_commit_oid.into(),
                new_commit_oid,
            })
            .collect_vec();
        let rewritten_oids_map: HashMap<NonZeroOid, MaybeZeroOid> =
            rewritten_oids.into_iter().collect();
        (rewritten_oids_map, events)
    };

    let is_spurious_event = rewrite_type == "amend" && repo.is_rebase_underway()?;
    if !is_spurious_event {
        let message_rewritten_commits = Pluralize {
            amount: rewritten_oids.len().try_into()?,
            singular: "rewritten commit",
            plural: "rewritten commits",
        }
        .to_string();
        writeln!(
            effects.get_output_stream(),
            "branchless: processing {}",
            message_rewritten_commits
        )?;
    }

    event_log_db.add_events(events)?;

    if repo
        .get_rebase_state_dir_path()
        .join(EXTRA_POST_REWRITE_FILE_NAME)
        .exists()
    {
        move_branches(git_run_info, &repo, event_tx_id, &rewritten_oids)?;
        check_out_new_head(effects, git_run_info, &repo, event_tx_id, &rewritten_oids)?;
    }

    let should_check_abandoned_commits = get_restack_warn_abandoned(&repo)?;
    if should_check_abandoned_commits && !is_spurious_event {
        let merge_base_db = MergeBaseDb::new(&conn)?;
        warn_abandoned(
            effects,
            &repo,
            &merge_base_db,
            &event_log_db,
            rewritten_oids.keys().copied(),
        )?;
    }

    Ok(())
}

#[instrument]
fn check_out_new_head(
    effects: &Effects,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    event_tx_id: EventTransactionId,
    rewritten_oids: &HashMap<NonZeroOid, MaybeZeroOid>,
) -> eyre::Result<()> {
    let checkout_target: Option<OsString> =
        match repo.find_reference(&OsString::from("ORIG_HEAD"))? {
            None => None,
            Some(orig_head_reference) => match orig_head_reference.get_target()? {
                ReferenceTarget::Direct {
                    oid: MaybeZeroOid::Zero,
                } => {
                    // `HEAD` was unborn, so keep it that way.
                    None
                }

                ReferenceTarget::Direct {
                    oid: MaybeZeroOid::NonZero(oid),
                } => match rewritten_oids.get(&oid) {
                    Some(MaybeZeroOid::NonZero(oid)) => {
                        // This OID was rewritten, so check out the new version of the commit.
                        Some(OsString::from(oid.to_string()))
                    }
                    Some(MaybeZeroOid::Zero) => {
                        // The commit was skipped. Get the new location for `HEAD`.
                        get_updated_head_oid(repo)?.map(|oid| oid.to_string().into())
                    }
                    None => {
                        // This OID was not rewritten, so check it out again.
                        Some(OsString::from(oid.to_string()))
                    }
                },

                ReferenceTarget::Symbolic { reference_name } => {
                    match repo.find_reference(&reference_name)? {
                        Some(reference) => {
                            // The branch may have been moved above, but
                            // regardless, we check it again out here.
                            Some(reference.get_name()?)
                        }
                        None => {
                            // The branch was deleted because it pointed to
                            // a skipped commit. Get the new location for
                            // `HEAD`.
                            get_updated_head_oid(repo)?.map(|oid| oid.to_string().into())
                        }
                    }
                }
            },
        };

    if let Some(checkout_target) = checkout_target {
        let exit_code = git_run_info.run(
            effects,
            Some(event_tx_id),
            &[&OsString::from("checkout"), &checkout_target],
        )?;
        if exit_code != 0 {
            eyre::bail!(
                "Failed to check out previous HEAD location: {:?}",
                &checkout_target
            );
        }
    }

    Ok(())
}

#[instrument(skip(old_commit_oids))]
fn warn_abandoned(
    effects: &Effects,
    repo: &Repo,
    merge_base_db: &MergeBaseDb,
    event_log_db: &EventLogDb,
    old_commit_oids: impl IntoIterator<Item = NonZeroOid>,
) -> eyre::Result<()> {
    // The caller will have added events to the event log database, so make sure
    // to construct a fresh `EventReplayer` here.
    let event_replayer = EventReplayer::from_event_log_db(effects, repo, event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();

    let head_oid = repo.get_head_info()?.oid;
    let main_branch_oid = repo.get_main_branch_oid()?;
    let branch_oid_to_names = repo.get_branch_oid_to_names()?;
    let graph = make_graph(
        effects,
        repo,
        merge_base_db,
        &event_replayer,
        event_cursor,
        &HeadOid(head_oid),
        &MainBranchOid(main_branch_oid),
        &BranchOids(branch_oid_to_names.keys().copied().collect()),
        false,
    )?;

    let (all_abandoned_children, all_abandoned_branches) = {
        let mut all_abandoned_children: HashSet<NonZeroOid> = HashSet::new();
        let mut all_abandoned_branches: HashSet<&OsStr> = HashSet::new();
        for old_commit_oid in old_commit_oids {
            let abandoned_result = find_abandoned_children(
                &graph,
                &event_replayer,
                event_replayer.make_default_cursor(),
                old_commit_oid,
            );
            let (_rewritten_oid, abandoned_children) = match abandoned_result {
                Some(abandoned_result) => abandoned_result,
                None => continue,
            };
            all_abandoned_children.extend(abandoned_children.iter());
            if let Some(branch_names) = branch_oid_to_names.get(&old_commit_oid) {
                all_abandoned_branches.extend(branch_names.iter().map(OsString::as_os_str));
            }
        }
        (all_abandoned_children, all_abandoned_branches)
    };
    let num_abandoned_children = all_abandoned_children.len();
    let num_abandoned_branches = all_abandoned_branches.len();

    if num_abandoned_children > 0 || num_abandoned_branches > 0 {
        let warning_items = {
            let mut warning_items = Vec::new();
            if num_abandoned_children > 0 {
                warning_items.push(
                    Pluralize {
                        amount: num_abandoned_children.try_into()?,
                        singular: "commit",
                        plural: "commits",
                    }
                    .to_string(),
                );
            }
            if num_abandoned_branches > 0 {
                let abandoned_branch_count = Pluralize {
                    amount: num_abandoned_branches.try_into()?,
                    singular: "branch",
                    plural: "branches",
                }
                .to_string();

                let mut all_abandoned_branches: Vec<String> = all_abandoned_branches
                    .iter()
                    .map(|branch_name| CategorizedReferenceName::new(branch_name).render_suffix())
                    .collect();
                all_abandoned_branches.sort_unstable();
                let abandoned_branches_list = all_abandoned_branches.join(", ");
                warning_items.push(format!(
                    "{} ({})",
                    abandoned_branch_count, abandoned_branches_list
                ));
            }

            warning_items
        };

        let warning_message = warning_items.join(" and ");
        let warning_message = style(format!("This operation abandoned {}!", warning_message))
            .bold()
            .yellow();

        print!(
            "\
branchless: {warning_message}
branchless: Consider running one of the following:
branchless:   - {git_restack}: re-apply the abandoned commits/branches
branchless:     (this is most likely what you want to do)
branchless:   - {git_smartlog}: assess the situation
branchless:   - {git_hide} [<commit>...]: hide the commits from the smartlog
branchless:   - {git_undo}: undo the operation
branchless:   - {config_command}: suppress this message
",
            warning_message = warning_message,
            git_smartlog = style("git smartlog").bold(),
            git_restack = style("git restack").bold(),
            git_hide = style("git hide").bold(),
            git_undo = style("git undo").bold(),
            config_command = style(format!(
                "git config {} false",
                RESTACK_WARN_ABANDONED_CONFIG_KEY
            ))
            .bold(),
        );
    }

    Ok(())
}

const EXTRA_POST_REWRITE_FILE_NAME: &str = "branchless_do_extra_post_rewrite";

/// Git stores a file called `orig-head` in the rebase state directory. This is
/// either a branch or an OID which is to be returned to after the rebase has
/// concluded, or if the rebase is aborted.
///
/// In order to handle the case of a commit being skipped and its corresponding
/// branch being deleted, we need to store our own copy of the original `HEAD`
/// OID, and then replace it once the rebase is about to conclude. We can't do
/// it earlier, because if the user aborts the rebase after the commit has been
/// skipped, then they would be returned to the wrong commit.
const UPDATED_HEAD_FILE_NAME: &str = "branchless_updated_head";

fn get_original_head_oid(repo: &Repo) -> eyre::Result<MaybeZeroOid> {
    let original_head_oid_file_path = repo.get_rebase_state_dir_path().join("orig-head");
    let original_head_oid_file_contents = std::fs::read_to_string(&original_head_oid_file_path)
        .wrap_err_with(|| {
            format!(
                "Reading `orig-head` from: {:?}",
                &original_head_oid_file_path
            )
        })?;
    let oid: MaybeZeroOid = original_head_oid_file_contents.parse()?;
    Ok(oid)
}

#[instrument]
fn save_updated_head_oid(repo: &Repo, updated_head_oid: NonZeroOid) -> eyre::Result<()> {
    let dest_file_name = repo
        .get_rebase_state_dir_path()
        .join(UPDATED_HEAD_FILE_NAME);
    std::fs::write(dest_file_name, updated_head_oid.to_string())?;
    Ok(())
}

#[instrument]
fn get_updated_head_oid(repo: &Repo) -> eyre::Result<Option<NonZeroOid>> {
    let source_file_name = repo
        .get_rebase_state_dir_path()
        .join(UPDATED_HEAD_FILE_NAME);
    match std::fs::read_to_string(source_file_name) {
        Ok(result) => Ok(Some(result.parse()?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// For rebases, register that extra cleanup actions should be taken when the
/// rebase finishes and calls the post-rewrite hook. We don't want to change the
/// behavior of `git rebase` itself, except when called via `git-branchless`, so
/// that the user's expectations aren't unexpectedly subverted.
pub fn hook_register_extra_post_rewrite_hook() -> eyre::Result<()> {
    let repo = Repo::from_current_dir()?;
    let file_name = repo
        .get_rebase_state_dir_path()
        .join(EXTRA_POST_REWRITE_FILE_NAME);
    File::create(file_name).wrap_err_with(|| "Registering extra post-rewrite hook")?;
    Ok(())
}

/// For rebases, detect empty commits (which have probably been applied
/// upstream) and write them to the `rewritten-list` file, so that they're later
/// passed to the `post-rewrite` hook.
pub fn hook_drop_commit_if_empty(
    effects: &Effects,
    old_commit_oid: NonZeroOid,
) -> eyre::Result<()> {
    let repo = Repo::from_current_dir()?;
    let head_info = repo.get_head_info()?;
    let head_oid = match head_info.oid {
        Some(head_oid) => head_oid,
        None => return Ok(()),
    };
    let head_commit = match repo.find_commit(head_oid)? {
        Some(head_commit) => head_commit,
        None => return Ok(()),
    };

    if !head_commit.is_empty() {
        return Ok(());
    }

    let only_parent_oid = match head_commit.get_only_parent_oid() {
        Some(only_parent_oid) => only_parent_oid,
        None => return Ok(()),
    };
    writeln!(
        effects.get_output_stream(),
        "Skipped now-empty commit: {}",
        printable_styled_string(effects.get_glyphs(), head_commit.friendly_describe()?)?
    )?;
    repo.set_head(only_parent_oid)?;

    let orig_head_oid = match repo.find_reference(&OsString::from("ORIG_HEAD"))? {
        Some(orig_head_reference) => orig_head_reference
            .peel_to_commit()?
            .map(|orig_head_commit| orig_head_commit.get_oid()),
        None => None,
    };
    if Some(old_commit_oid) == orig_head_oid {
        save_updated_head_oid(&repo, only_parent_oid)?;
    }
    add_rewritten_list_entries(
        &repo.get_rebase_state_dir_path().join("rewritten-list"),
        &[
            (old_commit_oid, MaybeZeroOid::Zero),
            (head_commit.get_oid(), MaybeZeroOid::Zero),
        ],
    )?;

    Ok(())
}

/// For rebases, if a commit is known to have been applied upstream, skip it
/// without attempting to apply it.
pub fn hook_skip_upstream_applied_commit(
    effects: &Effects,
    commit_oid: NonZeroOid,
) -> eyre::Result<()> {
    let repo = Repo::from_current_dir()?;
    let commit = match repo.find_commit(commit_oid)? {
        Some(commit) => commit,
        None => eyre::bail!("Could not find commit: {:?}", commit_oid),
    };
    writeln!(
        effects.get_output_stream(),
        "Skipping commit (was already applied upstream): {}",
        printable_styled_string(effects.get_glyphs(), commit.friendly_describe()?)?
    )?;

    let original_head_oid = get_original_head_oid(&repo)?;
    if MaybeZeroOid::NonZero(commit_oid) == original_head_oid {
        let current_head_oid = repo.get_head_info()?.oid;
        if let Some(current_head_oid) = current_head_oid {
            save_updated_head_oid(&repo, current_head_oid)?;
        }
    }
    add_rewritten_list_entries(
        &repo.get_rebase_state_dir_path().join("rewritten-list"),
        &[(commit_oid, MaybeZeroOid::Zero)],
    )?;

    Ok(())
}
