//! Hooks used to have Git call back into `git-branchless` for various functionality.

use std::collections::{HashMap, HashSet};

use std::fmt::Write;
use std::fs::{self, File};
use std::io::{self, stdin, BufRead, BufReader, Read, Write as WriteIo};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::SystemTime;

use console::style;
use eyre::Context;
use itertools::Itertools;
use tempfile::NamedTempFile;
use tracing::instrument;

use crate::core::check_out::CheckOutCommitOptions;
use crate::core::config::{get_hint_enabled, print_hint_suppression_notice, Hint};
use crate::core::dag::Dag;
use crate::core::effects::Effects;
use crate::core::eventlog::{Event, EventLogDb, EventReplayer};
use crate::core::formatting::Pluralize;
use crate::core::repo_ext::RepoExt;
use crate::git::{
    CategorizedReferenceName, GitRunInfo, MaybeZeroOid, NonZeroOid, ReferenceName, Repo,
    ResolvedReferenceInfo,
};

use super::execute::check_out_updated_head;
use super::{find_abandoned_children, move_branches};

/// Get the path to the file which stores the list of "deferred commits".
///
/// During a rebase, we make new commits, but if we abort the rebase, we don't
/// want those new commits to persist in the smartlog, etc. To address this, we
/// instead queue up the list of created commits and only confirm them once the
/// rebase has completed.
///
/// Note that this has the effect that if the user manually creates a commit
/// during a rebase, and then aborts the rebase, the commit will not be
/// available in the event log anywhere. This is probably acceptable.
pub fn get_deferred_commits_path(repo: &Repo) -> PathBuf {
    repo.get_rebase_state_dir_path().join("deferred-commits")
}

fn read_deferred_commits(repo: &Repo) -> eyre::Result<Vec<NonZeroOid>> {
    let deferred_commits_path = get_deferred_commits_path(repo);
    let contents = match fs::read_to_string(&deferred_commits_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => Default::default(),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("Reading deferred commits at {deferred_commits_path:?}"))
        }
    };
    let commit_oids = contents.lines().map(NonZeroOid::from_str).try_collect()?;
    Ok(commit_oids)
}

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
    tempfile_dir: &Path,
    rewritten_list_path: &Path,
    rewritten_oids: &[(NonZeroOid, MaybeZeroOid)],
) -> eyre::Result<()> {
    std::fs::create_dir_all(tempfile_dir).wrap_err("Creating tempfile dir")?;
    let mut tempfile =
        NamedTempFile::new_in(tempfile_dir).wrap_err("Creating temporary `rewritten-list` file")?;

    let file = tempfile.as_file_mut();
    for (old_commit_oid, new_commit_oid) in rewritten_oids {
        writeln!(file, "{old_commit_oid} {new_commit_oid}")?;
    }
    tempfile
        .persist(rewritten_list_path)
        .wrap_err("Moving new rewritten-list into place")?;
    Ok(())
}

#[instrument]
fn add_rewritten_list_entries(
    tempfile_dir: &Path,
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

    write_rewritten_list(tempfile_dir, rewritten_list_path, new_entries.as_slice())?;
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
    let is_spurious_event = rewrite_type == "amend" && repo.is_rebase_underway()?;
    if is_spurious_event {
        return Ok(());
    }

    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "hook-post-rewrite")?;

    {
        let deferred_commit_oids = read_deferred_commits(&repo)?;
        let commit_events = deferred_commit_oids
            .into_iter()
            .map(|commit_oid| Event::CommitEvent {
                timestamp,
                event_tx_id,
                commit_oid,
            })
            .collect_vec();
        event_log_db.add_events(commit_events)?;
    }

    let (rewritten_oids, rewrite_events) = {
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

    let message_rewritten_commits = Pluralize {
        determiner: None,
        amount: rewritten_oids.len(),
        unit: ("rewritten commit", "rewritten commits"),
    }
    .to_string();
    writeln!(
        effects.get_output_stream(),
        "branchless: processing {message_rewritten_commits}"
    )?;
    event_log_db.add_events(rewrite_events)?;

    if repo
        .get_rebase_state_dir_path()
        .join(EXTRA_POST_REWRITE_FILE_NAME)
        .exists()
    {
        // Make sure to resolve `ORIG_HEAD` before we potentially delete the
        // branch it points to, so that we can get the original OID of `HEAD`.
        let previous_head_info = load_original_head_info(&repo)?;
        move_branches(effects, git_run_info, &repo, event_tx_id, &rewritten_oids)?;

        let skipped_head_updated_oid = load_updated_head_oid(&repo)?;
        match check_out_updated_head(
            effects,
            git_run_info,
            &repo,
            &event_log_db,
            event_tx_id,
            &rewritten_oids,
            &previous_head_info,
            skipped_head_updated_oid,
            &CheckOutCommitOptions::default(),
        )? {
            Ok(()) => {}
            Err(_exit_code) => {
                eyre::bail!("Could not check out your updated `HEAD` commit.");
            }
        }
    }

    let should_check_abandoned_commits = get_hint_enabled(&repo, Hint::RestackWarnAbandoned)?;
    if should_check_abandoned_commits && !is_spurious_event {
        let printed_hint = warn_abandoned(
            effects,
            &repo,
            &conn,
            &event_log_db,
            rewritten_oids.keys().copied(),
        )?;
        if printed_hint {
            print_hint_suppression_notice(effects, Hint::RestackWarnAbandoned)?;
        }
    }

    Ok(())
}

#[instrument(skip(old_commit_oids))]
fn warn_abandoned(
    effects: &Effects,
    repo: &Repo,
    conn: &rusqlite::Connection,
    event_log_db: &EventLogDb,
    old_commit_oids: impl IntoIterator<Item = NonZeroOid>,
) -> eyre::Result<bool> {
    // The caller will have added events to the event log database, so make sure
    // to construct a fresh `EventReplayer` here.
    let references_snapshot = repo.get_references_snapshot()?;
    let event_replayer = EventReplayer::from_event_log_db(effects, repo, event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let dag = Dag::open_and_sync(
        effects,
        repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let (all_abandoned_children, all_abandoned_branches) = {
        let mut all_abandoned_children: HashSet<NonZeroOid> = HashSet::new();
        let mut all_abandoned_branches: HashSet<&str> = HashSet::new();
        for old_commit_oid in old_commit_oids {
            let abandoned_result =
                find_abandoned_children(&dag, &event_replayer, event_cursor, old_commit_oid)?;
            let (_rewritten_oid, abandoned_children) = match abandoned_result {
                Some(abandoned_result) => abandoned_result,
                None => continue,
            };
            all_abandoned_children.extend(abandoned_children.iter());
            if let Some(branch_names) = references_snapshot.branch_oid_to_names.get(&old_commit_oid)
            {
                all_abandoned_branches
                    .extend(branch_names.iter().map(|branch_name| branch_name.as_str()));
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
                        determiner: None,
                        amount: num_abandoned_children,
                        unit: ("commit", "commits"),
                    }
                    .to_string(),
                );
            }
            if num_abandoned_branches > 0 {
                let abandoned_branch_count = Pluralize {
                    determiner: None,
                    amount: num_abandoned_branches,
                    unit: ("branch", "branches"),
                }
                .to_string();

                let mut all_abandoned_branches: Vec<String> = all_abandoned_branches
                    .into_iter()
                    .map(|branch_name| {
                        CategorizedReferenceName::new(&branch_name.into()).render_suffix()
                    })
                    .collect();
                all_abandoned_branches.sort_unstable();
                let abandoned_branches_list = all_abandoned_branches.join(", ");
                warning_items.push(format!(
                    "{abandoned_branch_count} ({abandoned_branches_list})"
                ));
            }

            warning_items
        };

        let warning_message = warning_items.join(" and ");
        let warning_message = style(format!("This operation abandoned {warning_message}!"))
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
",
            warning_message = warning_message,
            git_smartlog = style("git smartlog").bold(),
            git_restack = style("git restack").bold(),
            git_hide = style("git hide").bold(),
            git_undo = style("git undo").bold(),
        );
        Ok(true)
    } else {
        Ok(false)
    }
}

const ORIGINAL_HEAD_OID_FILE_NAME: &str = "branchless_original_head_oid";
const ORIGINAL_HEAD_FILE_NAME: &str = "branchless_original_head";

/// Save the name of the currently checked-out branch. This should be called as
/// part of initializing the rebase.
#[instrument]
pub fn save_original_head_info(repo: &Repo, head_info: &ResolvedReferenceInfo) -> eyre::Result<()> {
    let ResolvedReferenceInfo {
        oid,
        reference_name,
    } = head_info;

    if let Some(oid) = oid {
        let dest_file_name = repo
            .get_rebase_state_dir_path()
            .join(ORIGINAL_HEAD_OID_FILE_NAME);
        std::fs::write(dest_file_name, oid.to_string()).wrap_err("Writing head OID")?;
    }

    if let Some(head_name) = reference_name {
        let dest_file_name = repo
            .get_rebase_state_dir_path()
            .join(ORIGINAL_HEAD_FILE_NAME);
        std::fs::write(dest_file_name, head_name.as_str()).wrap_err("Writing head name")?;
    }

    Ok(())
}

#[instrument]
fn load_original_head_info(repo: &Repo) -> eyre::Result<ResolvedReferenceInfo> {
    let head_oid = {
        let source_file_name = repo
            .get_rebase_state_dir_path()
            .join(ORIGINAL_HEAD_OID_FILE_NAME);
        match std::fs::read_to_string(source_file_name) {
            Ok(oid) => Some(oid.parse().wrap_err("Parsing original head OID")?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        }
    };

    let head_name = {
        let source_file_name = repo
            .get_rebase_state_dir_path()
            .join(ORIGINAL_HEAD_FILE_NAME);
        match std::fs::read(source_file_name) {
            Ok(reference_name) => Some(ReferenceName::from_bytes(reference_name)?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        }
    };

    Ok(ResolvedReferenceInfo {
        oid: head_oid,
        reference_name: head_name,
    })
}

const EXTRA_POST_REWRITE_FILE_NAME: &str = "branchless_do_extra_post_rewrite";

/// In order to handle the case of a commit being skipped and its corresponding
/// branch being deleted, we need to store our own copy of the original `HEAD`
/// OID, and then replace it once the rebase is about to conclude. We can't do
/// it earlier, because if the user aborts the rebase after the commit has been
/// skipped, then they would be returned to the wrong commit.
const UPDATED_HEAD_FILE_NAME: &str = "branchless_updated_head";

#[instrument]
fn save_updated_head_oid(repo: &Repo, updated_head_oid: NonZeroOid) -> eyre::Result<()> {
    let dest_file_name = repo
        .get_rebase_state_dir_path()
        .join(UPDATED_HEAD_FILE_NAME);
    std::fs::write(dest_file_name, updated_head_oid.to_string())?;
    Ok(())
}

#[instrument]
fn load_updated_head_oid(repo: &Repo) -> eyre::Result<Option<NonZeroOid>> {
    let source_file_name = repo
        .get_rebase_state_dir_path()
        .join(UPDATED_HEAD_FILE_NAME);
    match std::fs::read_to_string(source_file_name) {
        Ok(result) => Ok(Some(result.parse()?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Register extra cleanup actions for rebase.
///
/// For rebases, register that extra cleanup actions should be taken when the
/// rebase finishes and calls the post-rewrite hook. We don't want to change the
/// behavior of `git rebase` itself, except when called via `git-branchless`, so
/// that the user's expectations aren't unexpectedly subverted.
pub fn hook_register_extra_post_rewrite_hook() -> eyre::Result<()> {
    let repo = Repo::from_current_dir()?;
    let file_name = repo
        .get_rebase_state_dir_path()
        .join(EXTRA_POST_REWRITE_FILE_NAME);
    File::create(file_name).wrap_err("Registering extra post-rewrite hook")?;

    // This is the last step before the rebase concludes. Ordinarily, Git will
    // use `head-name` as the name of the previously checked-out branch, and
    // move that branch to point to the current commit (and check it out again).
    // We want to suppress this behavior because we don't want the branch to
    // move (or, if we do want it to move, we will handle that ourselves as part
    // of the post-rewrite hook). So we update `head-name` to contain "detached
    // HEAD" to indicate to Git that no branch was checked out prior to the
    // rebase, so that it doesn't try to adjust any branches.
    std::fs::write(
        repo.get_rebase_state_dir_path().join("head-name"),
        "detached HEAD",
    )
    .wrap_err("Setting `head-name` to detached HEAD")?;

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
        effects
            .get_glyphs()
            .render(head_commit.friendly_describe(effects.get_glyphs())?)?
    )?;
    repo.set_head(only_parent_oid)?;

    let orig_head_oid = match repo.find_reference(&"ORIG_HEAD".into())? {
        Some(orig_head_reference) => orig_head_reference
            .peel_to_commit()?
            .map(|orig_head_commit| orig_head_commit.get_oid()),
        None => None,
    };
    if Some(old_commit_oid) == orig_head_oid {
        save_updated_head_oid(&repo, only_parent_oid)?;
    }
    add_rewritten_list_entries(
        &repo.get_tempfile_dir()?,
        &repo.get_rebase_state_dir_path().join("rewritten-list"),
        &[
            (old_commit_oid, MaybeZeroOid::Zero),
            (head_commit.get_oid(), MaybeZeroOid::Zero),
        ],
    )?;

    Ok(())
}

/// For rebases, update the status of a commit that is known to have been
/// applied upstream. It can either be skipped entirely (when called with
/// `MaybeZeroOid::Zero`) or be marked as having been rewritten to a
/// different commit entirely.
pub fn hook_skip_upstream_applied_commit(
    effects: &Effects,
    commit_oid: NonZeroOid,
    rewritten_oid: MaybeZeroOid,
) -> eyre::Result<()> {
    let repo = Repo::from_current_dir()?;
    let commit = repo.find_commit_or_fail(commit_oid)?;
    writeln!(
        effects.get_output_stream(),
        "Skipping commit (was already applied upstream): {}",
        effects
            .get_glyphs()
            .render(commit.friendly_describe(effects.get_glyphs())?)?
    )?;

    if let Some(orig_head_reference) = repo.find_reference(&"ORIG_HEAD".into())? {
        let resolved_orig_head = repo.resolve_reference(&orig_head_reference)?;
        if let Some(original_head_oid) = resolved_orig_head.oid {
            if original_head_oid == commit_oid {
                let current_head_oid = repo.get_head_info()?.oid;
                if let Some(current_head_oid) = current_head_oid {
                    save_updated_head_oid(&repo, current_head_oid)?;
                }
            }
        }
    }
    add_rewritten_list_entries(
        &repo.get_tempfile_dir()?,
        &repo.get_rebase_state_dir_path().join("rewritten-list"),
        &[(commit_oid, rewritten_oid)],
    )?;

    Ok(())
}
