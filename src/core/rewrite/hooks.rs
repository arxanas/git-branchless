//! Hooks used to have Git call back into `git-branchless` for various functionality.

use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{stdin, BufRead};
use std::time::SystemTime;

use anyhow::Context;
use console::style;
use fn_error_context::context;

use crate::core::config::{get_restack_warn_abandoned, RESTACK_WARN_ABANDONED_CONFIG_KEY};
use crate::core::eventlog::{Event, EventLogDb, EventReplayer};
use crate::core::formatting::{printable_styled_string, Glyphs, Pluralize};
use crate::core::graph::{make_graph, BranchOids, HeadOid, MainBranchOid};
use crate::core::mergebase::MergeBaseDb;
use crate::git::{CategorizedReferenceName, GitRunInfo, MaybeZeroOid, NonZeroOid, Repo};

use super::{find_abandoned_children, move_branches};

/// Handle Git's `post-rewrite` hook.
///
/// See the man-page for `githooks(5)`.
#[context("Processing post-rewrite hook")]
pub fn hook_post_rewrite(git_run_info: &GitRunInfo, rewrite_type: &str) -> anyhow::Result<()> {
    let now = SystemTime::now();
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?.as_secs_f64();

    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let mut event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "hook-post-rewrite")?;

    let (rewritten_oids, events) = {
        let mut rewritten_oids = HashMap::new();
        let mut events = Vec::new();
        for line in stdin().lock().lines() {
            let line = line?;
            let line = line.trim();
            match *line.split(' ').collect::<Vec<_>>().as_slice() {
                [old_commit_oid, new_commit_oid, ..] => {
                    let old_commit_oid: NonZeroOid = old_commit_oid.parse()?;
                    let new_commit_oid: MaybeZeroOid = new_commit_oid.parse()?;

                    rewritten_oids.insert(old_commit_oid, new_commit_oid);
                    events.push(Event::RewriteEvent {
                        timestamp,
                        event_tx_id,
                        old_commit_oid: old_commit_oid.into(),
                        new_commit_oid,
                    })
                }
                _ => anyhow::bail!("Invalid rewrite line: {:?}", &line),
            }
        }
        (rewritten_oids, events)
    };

    let is_spurious_event = rewrite_type == "amend" && repo.is_rebase_underway()?;
    if !is_spurious_event {
        let message_rewritten_commits = Pluralize {
            amount: rewritten_oids.len().try_into()?,
            singular: "rewritten commit",
            plural: "rewritten commits",
        }
        .to_string();
        println!("branchless: processing {}", message_rewritten_commits);
    }

    event_log_db.add_events(events)?;

    if repo
        .get_rebase_state_dir_path()
        .join(EXTRA_POST_REWRITE_FILE_NAME)
        .exists()
    {
        move_branches(git_run_info, &repo, event_tx_id, &rewritten_oids)?;
    }

    let should_check_abandoned_commits = get_restack_warn_abandoned(&repo)?;
    if should_check_abandoned_commits && !is_spurious_event {
        let merge_base_db = MergeBaseDb::new(&conn)?;
        warn_abandoned(
            &repo,
            &merge_base_db,
            &event_log_db,
            rewritten_oids.keys().copied(),
        )?;
    }

    Ok(())
}

#[context("Warning about abandoned commits/branches")]
fn warn_abandoned(
    repo: &Repo,
    merge_base_db: &MergeBaseDb,
    event_log_db: &EventLogDb,
    old_commit_oids: impl IntoIterator<Item = NonZeroOid>,
) -> anyhow::Result<()> {
    // The caller will have added events to the event log database, so make sure
    // to construct a fresh `EventReplayer` here.
    let event_replayer = EventReplayer::from_event_log_db(repo, event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();

    let head_oid = repo.get_head_info()?.oid;
    let main_branch_oid = repo.get_main_branch_oid()?;
    let branch_oid_to_names = repo.get_branch_oid_to_names()?;
    let graph = make_graph(
        &repo,
        &merge_base_db,
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

/// For rebases, register that extra cleanup actions should be taken when the
/// rebase finishes and calls the post-rewrite hook. We don't want to change the
/// behavior of `git rebase` itself, except when called via `git-branchless`, so
/// that the user's expectations aren't unexpectedly subverted.
pub fn hook_register_extra_post_rewrite_hook() -> anyhow::Result<()> {
    let repo = Repo::from_current_dir()?;
    let file_name = repo
        .get_rebase_state_dir_path()
        .join(EXTRA_POST_REWRITE_FILE_NAME);
    File::create(file_name).with_context(|| "Registering extra post-rewrite hook")?;
    Ok(())
}

/// For rebases, detect empty commits (which have probably been applied
/// upstream) and write them to the `rewritten-list` file, so that they're later
/// passed to the `post-rewrite` hook.
pub fn hook_drop_commit_if_empty(old_commit_oid: NonZeroOid) -> anyhow::Result<()> {
    let glyphs = Glyphs::detect();
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
    println!(
        "Skipped now-empty commit: {}",
        printable_styled_string(&glyphs, head_commit.friendly_describe()?)?
    );
    repo.set_head(only_parent_oid)?;
    repo.add_rewritten_list_entries(&[
        (old_commit_oid, MaybeZeroOid::Zero),
        // NB: from the user's perspective, they don't need to know about the empty
        // commit that was created. It might be better to edit the `rewritten-list`
        // and remove the entry which rewrote the old commit into the current `HEAD`
        // commit, rather than hiding the newly created `HEAD` commit.
        (head_commit.get_oid(), MaybeZeroOid::Zero),
    ])?;
    Ok(())
}

/// For rebases, if a commit is known to have been applied upstream, skip it
/// without attempting to apply it.
pub fn hook_skip_upstream_applied_commit(commit_oid: NonZeroOid) -> anyhow::Result<()> {
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let commit = match repo.find_commit(commit_oid)? {
        Some(commit) => commit,
        None => anyhow::bail!("Could not find commit: {:?}", commit_oid),
    };
    println!(
        "Skipping commit (was already applied upstream): {}",
        printable_styled_string(&glyphs, commit.friendly_describe()?)?
    );
    repo.add_rewritten_list_entries(&[(commit_oid, MaybeZeroOid::Zero)])?;
    Ok(())
}
