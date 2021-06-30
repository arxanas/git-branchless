//! Utilities to deal with rewritten commits. See `Event::RewriteEvent` for
//! specifics on commit rewriting.

use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::time::SystemTime;

use anyhow::Context;
use cursive::utils::markup::StyledString;
use fn_error_context::context;
use indicatif::{ProgressBar, ProgressStyle};

use crate::commands::gc::mark_commit_reachable;
use crate::core::formatting::printable_styled_string;
use crate::util::{get_branch_oid_to_names, get_repo_head, run_git, run_hook, GitRunInfo};

use super::eventlog::{Event, EventCursor, EventReplayer, EventTransactionId};
use super::formatting::Glyphs;
use super::graph::{find_path_to_merge_base, CommitGraph, MainBranchOid};
use super::mergebase::MergeBaseDb;
use super::metadata::{render_commit_metadata, CommitMessageProvider, CommitOidProvider};
use super::repo::Repo;

/// For a rewritten commit, find the newest version of the commit.
///
/// For example, if we amend commit `abc` into commit `def1`, and then amend
/// `def1` into `def2`, then we can traverse the event log to find out that `def2`
/// is the newest version of `abc`.
///
/// If a commit was rewritten into itself through some chain of events, then
/// returns `None`, rather than the same commit OID.
pub fn find_rewrite_target(
    graph: &CommitGraph,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    oid: git2::Oid,
) -> Option<git2::Oid> {
    let event = event_replayer.get_cursor_commit_latest_event(event_cursor, oid);
    let event = match event {
        Some(event) => event,
        None => return None,
    };
    match event {
        Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid,
            new_commit_oid,
        } => {
            if *old_commit_oid == oid && *new_commit_oid != oid {
                let possible_newer_oid =
                    find_rewrite_target(graph, event_replayer, event_cursor, *new_commit_oid);
                match possible_newer_oid {
                    Some(newer_commit_oid) => Some(newer_commit_oid),
                    None => Some(*new_commit_oid),
                }
            } else {
                None
            }
        }

        Event::RefUpdateEvent { .. }
        | Event::CommitEvent { .. }
        | Event::HideEvent { .. }
        | Event::UnhideEvent { .. } => None,
    }
}

/// Find commits which have been "abandoned" in the commit graph.
///
/// A commit is considered "abandoned" if it is visible, but one of its parents
/// is hidden.
pub fn find_abandoned_children(
    graph: &CommitGraph,
    event_replayer: &EventReplayer,
    event_cursor: EventCursor,
    oid: git2::Oid,
) -> Option<(git2::Oid, Vec<git2::Oid>)> {
    let rewritten_oid = find_rewrite_target(graph, event_replayer, event_cursor, oid)?;

    // Adjacent main branch commits are not linked in the commit graph, but if
    // the user rewrote a main branch commit, then we may need to restack
    // subsequent main branch commits. Find the real set of children commits so
    // that we can do this.
    let mut real_children_oids = graph[&oid].children.clone();
    let additional_children_oids: HashSet<git2::Oid> = graph
        .iter()
        .filter_map(|(possible_child_oid, possible_child_node)| {
            if real_children_oids.contains(possible_child_oid) {
                // Don't bother looking up the parents for commits we are
                // already including.
                None
            } else if possible_child_node
                .commit
                .parent_ids()
                .any(|parent_oid| parent_oid == oid)
            {
                Some(possible_child_oid)
            } else {
                None
            }
        })
        .copied()
        .collect();
    real_children_oids.extend(additional_children_oids);

    let visible_children_oids = real_children_oids
        .iter()
        .filter(|child_oid| graph[child_oid].is_visible)
        .copied()
        .collect();
    Some((rewritten_oid, visible_children_oids))
}

#[derive(Debug)]
enum RebaseCommand {
    Label { label_name: String },
    Reset { label_name: String },
    Pick { commit_oid: git2::Oid },
}

/// Represents a sequence of commands that can be executed to carry out a rebase
/// operation.
#[derive(Debug)]
pub struct RebasePlan {
    first_dest_oid: git2::Oid,
    commands: Vec<RebaseCommand>,
}

impl ToString for RebaseCommand {
    fn to_string(&self) -> String {
        match self {
            RebaseCommand::Label { label_name } => format!("label {}", label_name),
            RebaseCommand::Reset { label_name } => format!("reset {}", label_name),
            RebaseCommand::Pick { commit_oid } => format!("pick {}", commit_oid),
        }
    }
}

/// Builder for a rebase plan. Unlike regular Git rebases, a `git-branchless`
/// rebase plan can move multiple unrelated subtrees to unrelated destinations.
pub struct RebasePlanBuilder<'repo> {
    repo: &'repo git2::Repository,
    graph: &'repo CommitGraph<'repo>,
    merge_base_db: &'repo MergeBaseDb<'repo>,
    main_branch_oid: git2::Oid,

    first_dest_oid: Option<git2::Oid>,
    commands: Vec<RebaseCommand>,
    used_labels: HashSet<String>,
}

impl<'repo> RebasePlanBuilder<'repo> {
    /// Constructor.
    pub fn new(
        repo: &'repo git2::Repository,
        graph: &'repo CommitGraph,
        merge_base_db: &'repo MergeBaseDb,
        main_branch_oid: &MainBranchOid,
    ) -> Self {
        let MainBranchOid(main_branch_oid) = main_branch_oid;
        RebasePlanBuilder {
            repo,
            graph,
            merge_base_db,
            main_branch_oid: *main_branch_oid,
            first_dest_oid: Default::default(),
            commands: Default::default(),
            used_labels: Default::default(),
        }
    }

    fn make_label_name(&mut self, preferred_name: impl Into<String>) -> anyhow::Result<String> {
        let mut preferred_name = preferred_name.into();
        if !self.used_labels.contains(&preferred_name) {
            self.used_labels.insert(preferred_name.clone());
            Ok(preferred_name)
        } else {
            preferred_name.push('\'');
            self.make_label_name(preferred_name)
        }
    }

    fn make_rebase_plan_for_current_commit(
        &mut self,
        current_oid: git2::Oid,
        current_label: &str,
        mut acc: Vec<RebaseCommand>,
    ) -> anyhow::Result<Vec<RebaseCommand>> {
        let acc = {
            acc.push(RebaseCommand::Pick {
                commit_oid: current_oid,
            });
            acc
        };
        let current_node = match self.graph.get(&current_oid) {
            Some(current_node) => current_node,
            None => {
                anyhow::bail!(format!(
                    "BUG: commit {} could not be found in the commit graph",
                    current_oid.to_string()
                ))
            }
        };

        match current_node.children.as_slice() {
            [] => Ok(acc),
            [only_child_oid] => {
                let acc =
                    self.make_rebase_plan_for_current_commit(*only_child_oid, current_label, acc)?;
                Ok(acc)
            }
            children => {
                let command_num = acc.len();
                let label_name = self.make_label_name(format!("label-{}", command_num))?;
                let mut acc = acc;
                acc.push(RebaseCommand::Label {
                    label_name: label_name.clone(),
                });
                for child_oid in children {
                    acc = self.make_rebase_plan_for_current_commit(*child_oid, &label_name, acc)?;
                    acc.push(RebaseCommand::Reset {
                        label_name: label_name.clone(),
                    });
                }
                Ok(acc)
            }
        }
    }

    /// Generate a sequence of rebase steps that cause the subtree at `source_oid`
    /// to be rebased on top of `dest_oid`.
    pub fn move_subtree(
        &mut self,
        source_oid: git2::Oid,
        dest_oid: git2::Oid,
    ) -> anyhow::Result<()> {
        let mut commands = vec![
            // First, move to the destination OID.
            RebaseCommand::Reset {
                label_name: dest_oid.to_string(),
            },
        ];

        let label_name = self.make_label_name("subtree")?;
        let (source_oid, main_branch_commands) = {
            let merge_base_oid = self.merge_base_db.get_merge_base_oid(
                self.repo,
                self.main_branch_oid,
                source_oid,
            )?;
            if merge_base_oid == Some(source_oid) {
                // In this case, the `source` OID is an ancestor of the main branch.
                // This means that the user is trying to rewrite public history,
                // which is typically not recommended, but let's try to do it
                // anyways.
                let path = find_path_to_merge_base(
                    &self.repo,
                    &self.merge_base_db,
                    self.main_branch_oid,
                    source_oid,
                )?
                .unwrap_or_default();
                let commands: Vec<RebaseCommand> = path
                    .into_iter()
                    // Skip the first element, which is the main branch OID, since
                    // it'll be picked as part of the recursive rebase plan below.
                    .skip(1)
                    // Reverse the path, since it goes from main branch OID to
                    // source OID, but we want a path from source OID to main branch
                    // OID.
                    .rev()
                    .map(|main_branch_commit| RebaseCommand::Pick {
                        commit_oid: main_branch_commit.id(),
                    })
                    .collect();
                (self.main_branch_oid, commands)
            } else {
                (source_oid, Vec::new())
            }
        };
        commands.extend(main_branch_commands);

        let commands =
            self.make_rebase_plan_for_current_commit(source_oid, &label_name, commands)?;
        self.first_dest_oid.get_or_insert(dest_oid);
        self.commands.extend(commands);
        Ok(())
    }

    /// Create the rebase plan. Returns `None` if there were no commands in the rebase plan.
    pub fn build(self) -> Option<RebasePlan> {
        let first_dest_oid = self.first_dest_oid?;
        Some(RebasePlan {
            first_dest_oid,
            commands: self.commands,
        })
    }
}

enum RebaseInMemoryResult {
    Succeeded {
        rewritten_oids: Vec<(git2::Oid, git2::Oid)>,
    },
    CannotRebaseMergeCommit {
        commit_oid: git2::Oid,
    },
    MergeConflict {
        commit_oid: git2::Oid,
    },
}

#[context("Updating commit timestamp")]
fn update_signature_timestamp(
    now: SystemTime,
    signature: git2::Signature,
) -> anyhow::Result<git2::Signature> {
    let seconds: i64 = now
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs()
        .try_into()?;
    let time = git2::Time::new(seconds, signature.when().offset_minutes());
    let name = match signature.name() {
        Some(name) => name,
        None => anyhow::bail!(
            "Could not decode signature name: {:?}",
            signature.name_bytes()
        ),
    };
    let email = match signature.email() {
        Some(email) => email,
        None => anyhow::bail!(
            "Could not decode signature email: {:?}",
            signature.email_bytes()
        ),
    };
    let signature = git2::Signature::new(name, email, &time)?;
    Ok(signature)
}

#[context("Rebasing in memory")]
fn rebase_in_memory(
    glyphs: &Glyphs,
    repo: &git2::Repository,
    rebase_plan: &RebasePlan,
    options: &ExecuteRebasePlanOptions,
) -> anyhow::Result<RebaseInMemoryResult> {
    let ExecuteRebasePlanOptions {
        now,
        // Transaction ID will be passed to the `post-rewrite` hook via
        // environment variable.
        event_tx_id: _,
        preserve_timestamps,
        force_in_memory: _,
        force_on_disk: _,
    } = options;

    let mut current_oid = rebase_plan.first_dest_oid;
    let mut labels: HashMap<String, git2::Oid> = HashMap::new();
    let mut rewritten_oids = Vec::new();

    let mut i = 0;
    let num_picks = rebase_plan
        .commands
        .iter()
        .filter(|command| match command {
            RebaseCommand::Label { .. } | RebaseCommand::Reset { .. } => false,
            RebaseCommand::Pick { .. } => true,
        })
        .count();

    for command in rebase_plan.commands.iter() {
        match command {
            RebaseCommand::Label { label_name } => {
                labels.insert(label_name.clone(), current_oid);
            }
            RebaseCommand::Reset { label_name } => {
                current_oid = match labels.get(label_name) {
                    Some(oid) => *oid,
                    None => match label_name.parse::<git2::Oid>() {
                        Ok(oid) => oid,
                        Err(_) => anyhow::bail!("BUG: no associated OID for label: {}", label_name),
                    },
                };
            }
            RebaseCommand::Pick { commit_oid } => {
                let current_commit = repo
                    .find_commit(current_oid)
                    .with_context(|| format!("Finding current commit by OID: {:?}", current_oid))?;
                let commit_to_apply = repo
                    .find_commit(*commit_oid)
                    .with_context(|| format!("Finding commit to apply by OID: {:?}", commit_oid))?;
                i += 1;

                let commit_description =
                    printable_styled_string(glyphs, friendly_describe_commit(repo, *commit_oid)?)?;
                let template = format!("[{}/{}] {{spinner}} {{wide_msg}}", i, num_picks);
                let progress = ProgressBar::new_spinner();
                progress.set_style(ProgressStyle::default_spinner().template(&template.trim()));
                progress.set_message("Starting");
                progress.enable_steady_tick(100);

                if commit_to_apply.parent_count() > 1 {
                    return Ok(RebaseInMemoryResult::CannotRebaseMergeCommit {
                        commit_oid: *commit_oid,
                    });
                };

                progress.set_message(format!("Applying patch for commit: {}", commit_description));
                let mut rebased_index =
                    repo.cherrypick_commit(&commit_to_apply, &current_commit, 0, None)?;

                progress.set_message(format!(
                    "Checking for merge conflicts: {}",
                    commit_description
                ));
                if rebased_index.has_conflicts() {
                    return Ok(RebaseInMemoryResult::MergeConflict {
                        commit_oid: *commit_oid,
                    });
                }

                progress.set_message(format!(
                    "Writing commit data to disk: {}",
                    commit_description
                ));
                let commit_tree_oid = rebased_index
                    .write_tree_to(repo)
                    .with_context(|| "Converting index to tree")?;
                let commit_tree = repo
                    .find_tree(commit_tree_oid)
                    .with_context(|| "Looking up freshly-written tree")?;
                let commit_message = match commit_to_apply.message_raw() {
                    Some(message) => message,
                    None => anyhow::bail!(
                        "Could not decode commit message for commit: {:?}",
                        commit_oid
                    ),
                };

                progress.set_message(format!("Committing to repository: {}", commit_description));
                let committer_signature = if *preserve_timestamps {
                    commit_to_apply.committer()
                } else {
                    update_signature_timestamp(*now, commit_to_apply.committer())?
                };
                let rebased_commit_oid = repo
                    .commit(
                        None,
                        &commit_to_apply.author(),
                        &committer_signature,
                        commit_message,
                        &commit_tree,
                        &[&current_commit],
                    )
                    .with_context(|| "Applying rebased commit")?;
                rewritten_oids.push((*commit_oid, rebased_commit_oid));
                current_oid = rebased_commit_oid;

                let commit_description = printable_styled_string(
                    glyphs,
                    friendly_describe_commit(repo, rebased_commit_oid)?,
                )?;
                progress.finish_with_message(format!("Committed as: {}", commit_description));
            }
        }
    }

    Ok(RebaseInMemoryResult::Succeeded { rewritten_oids })
}

/// Given a list of rewritten OIDs, move the branches attached to those OIDs
/// from their old commits to their new commits. Invoke the
/// `reference-transaction` hook when done.
pub fn move_branches<'a>(
    git_run_info: &GitRunInfo,
    repo: &'a Repo,
    event_tx_id: EventTransactionId,
    rewritten_oids_map: &'a HashMap<git2::Oid, git2::Oid>,
) -> anyhow::Result<()> {
    let branch_oid_to_names = get_branch_oid_to_names(repo)?;

    // We may experience an error in the case of a branch move. Ideally, we
    // would use `git2::Transaction::commit`, which stops the transaction at the
    // first error, but we don't know which references we successfully committed
    // in that case. Instead, we just do things non-atomically and record which
    // ones succeeded. See https://github.com/libgit2/libgit2/issues/5918
    let mut branch_moves: Vec<(git2::Oid, git2::Oid, &str)> = Vec::new();
    let mut branch_move_err: Option<anyhow::Error> = None;
    'outer: for (old_oid, names) in branch_oid_to_names.iter() {
        let new_oid = match rewritten_oids_map.get(&old_oid) {
            Some(new_oid) => new_oid,
            None => continue,
        };
        let new_commit = match repo.find_commit(*new_oid) {
            Ok(commit) => commit,
            Err(err) => {
                branch_move_err = Some(err.into());
                break 'outer;
            }
        };

        let mut names: Vec<_> = names.iter().collect();
        // Sort for determinism in tests.
        names.sort_unstable();
        for name in names {
            if let Err(err) = repo.branch(name, &new_commit, true) {
                branch_move_err = Some(err.into());
                break 'outer;
            }
            branch_moves.push((*old_oid, *new_oid, name))
        }
    }

    let branch_moves_stdin: String = branch_moves
        .into_iter()
        .map(|(old_oid, new_oid, name)| {
            format!("{} {} {}\n", old_oid.to_string(), new_oid.to_string(), name)
        })
        .collect();
    run_hook(
        git_run_info,
        repo,
        "reference-transaction",
        event_tx_id,
        &["committed"],
        Some(branch_moves_stdin),
    )?;
    match branch_move_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn post_rebase_in_memory(
    git_run_info: &GitRunInfo,
    repo: &Repo,
    rewritten_oids: &[(git2::Oid, git2::Oid)],
    options: &ExecuteRebasePlanOptions,
) -> anyhow::Result<isize> {
    let ExecuteRebasePlanOptions {
        now: _,
        event_tx_id,
        preserve_timestamps: _,
        force_in_memory: _,
        force_on_disk: _,
    } = options;

    // Note that if an OID has been mapped to multiple other OIDs, then the last
    // mapping wins. (This corresponds to the last applied rebase operation.)
    let rewritten_oids_map: HashMap<git2::Oid, git2::Oid> =
        rewritten_oids.iter().copied().collect();

    for new_oid in rewritten_oids_map.values() {
        mark_commit_reachable(repo, *new_oid)?;
    }

    let head = get_repo_head(repo)?;
    let head_branch = head
        .symbolic_target()
        .and_then(|s| s.strip_prefix("refs/heads/"));
    let head_oid = head.peel_to_commit()?.id();
    // Avoid moving the branch which HEAD points to, or else the index will show
    // a lot of changes in the working copy.
    repo.set_head_detached(head_oid)?;

    move_branches(git_run_info, repo, *event_tx_id, &rewritten_oids_map)?;

    // Call the `post-rewrite` hook only after moving branches so that we don't
    // produce a spurious abandoned-branch warning.
    let post_rewrite_stdin: String = rewritten_oids
        .iter()
        .map(|(old_oid, new_oid)| format!("{} {}\n", old_oid.to_string(), new_oid.to_string()))
        .collect();
    run_hook(
        git_run_info,
        repo,
        "post-rewrite",
        *event_tx_id,
        &["rebase"],
        Some(post_rewrite_stdin),
    )?;

    if let Some(new_head_oid) = rewritten_oids_map.get(&head_oid) {
        let head_target = match head_branch {
            Some(head_branch) => head_branch.to_string(),
            None => new_head_oid.to_string(),
        };
        let result = run_git(
            git_run_info,
            Some(*event_tx_id),
            &["checkout", &head_target],
        )?;
        if result != 0 {
            return Ok(result);
        }
    }

    Ok(0)
}

/// Rebase on-disk. We don't use `git2`'s `Rebase` machinery because it ends up
/// being too slow.
#[context("Rebasing on disk")]
fn rebase_on_disk(
    git_run_info: &GitRunInfo,
    repo: &git2::Repository,
    rebase_plan: &RebasePlan,
    options: &ExecuteRebasePlanOptions,
) -> anyhow::Result<isize> {
    let ExecuteRebasePlanOptions {
        // `git rebase` will make its own timestamp.
        now: _,
        event_tx_id,
        preserve_timestamps,
        force_in_memory: _,
        force_on_disk: _,
    } = options;

    let progress = ProgressBar::new_spinner();
    progress.enable_steady_tick(100);
    progress.set_message("Initializing rebase");

    let head = repo.head().with_context(|| "Getting repo `HEAD`")?;
    let orig_head_name = head.shorthand_bytes();

    let current_operation_type = {
        use git2::RepositoryState::*;
        match repo.state() {
            Clean | Bisect => None,
            Merge => Some("merge"),
            Revert | RevertSequence => Some("revert"),
            CherryPick | CherryPickSequence => Some("cherry-pick"),
            Rebase | RebaseInteractive | RebaseMerge => Some("rebase"),
            ApplyMailbox | ApplyMailboxOrRebase => Some("am"),
        }
    };
    if let Some(current_operation_type) = current_operation_type {
        progress.finish_and_clear();
        println!(
            "A {} operation is already in progress.",
            current_operation_type
        );
        println!(
            "Run git {0} --continue or git {0} --abort to resolve it and proceed.",
            current_operation_type
        );
        return Ok(1);
    }

    let repo_head_file_path = repo.path().join("HEAD");
    let orig_head_file_path = repo.path().join("ORIG_HEAD");
    std::fs::copy(&repo_head_file_path, &orig_head_file_path)
        .with_context(|| format!("Copying `HEAD` to: {:?}", &orig_head_file_path))?;

    let rebase_merge_dir = repo.path().join("rebase-merge");
    std::fs::create_dir_all(&rebase_merge_dir).with_context(|| {
        format!(
            "Creating rebase-merge directory at: {:?}",
            &rebase_merge_dir
        )
    })?;

    // Mark this rebase as an interactive rebase. For whatever reason, if this
    // is not marked as an interactive rebase, then some rebase plans fail with
    // this error:
    //
    // ```
    // BUG: builtin/rebase.c:1178: Unhandled rebase type 1
    // ```
    let interactive_file_path = rebase_merge_dir.join("interactive");
    std::fs::write(&interactive_file_path, "")
        .with_context(|| format!("Writing interactive to: {:?}", &interactive_file_path))?;

    // `head-name` appears to be purely for UX concerns. Git will warn if the
    // file isn't found.
    let head_name_file_path = rebase_merge_dir.join("head-name");
    std::fs::write(&head_name_file_path, orig_head_name)
        .with_context(|| format!("Writing head-name to: {:?}", &head_name_file_path))?;

    // Dummy `head` file. We will `reset` to the appropriate commit as soon as
    // we start the rebase.
    let rebase_merge_head_file_path = rebase_merge_dir.join("head");
    std::fs::write(
        &rebase_merge_head_file_path,
        rebase_plan.first_dest_oid.to_string(),
    )
    .with_context(|| format!("Writing head to: {:?}", &rebase_merge_head_file_path))?;

    // Dummy `onto` file. We may be rebasing onto a set of unrelated
    // nodes in the same operation, so there may not be a single "onto" node to
    // refer to.
    let onto_file_path = rebase_merge_dir.join("onto");
    std::fs::write(&onto_file_path, rebase_plan.first_dest_oid.to_string()).with_context(|| {
        format!(
            "Writing onto {:?} to: {:?}",
            &rebase_plan.first_dest_oid, &onto_file_path
        )
    })?;

    let todo_file_path = rebase_merge_dir.join("git-rebase-todo");
    std::fs::write(
        &todo_file_path,
        rebase_plan
            .commands
            .iter()
            .map(|command| format!("{}\n", command.to_string()))
            .collect::<String>(),
    )
    .with_context(|| {
        format!(
            "Writing `git-rebase-todo` to: {:?}",
            todo_file_path.as_path()
        )
    })?;

    let end_file_path = rebase_merge_dir.join("end");
    std::fs::write(
        end_file_path.as_path(),
        format!("{}\n", rebase_plan.commands.len()),
    )
    .with_context(|| format!("Writing `end` to: {:?}", end_file_path.as_path()))?;

    if *preserve_timestamps {
        let cdate_is_adate_file_path = rebase_merge_dir.join("cdate_is_adate");
        std::fs::write(&cdate_is_adate_file_path, "")
            .with_context(|| "Writing `cdate_is_adate` option file")?;
    }

    progress.finish_and_clear();
    println!("Calling Git for on-disk rebase...");
    let result = run_git(&git_run_info, Some(*event_tx_id), &["rebase", "--continue"])?;

    Ok(result)
}

#[context("Describing commit {}", commit_oid.to_string())]
fn friendly_describe_commit(
    repo: &git2::Repository,
    commit_oid: git2::Oid,
) -> anyhow::Result<StyledString> {
    let commit = repo
        .find_commit(commit_oid)
        .with_context(|| "Looking up commit to describe")?;
    let description = render_commit_metadata(
        &commit,
        &mut [
            &mut CommitOidProvider::new(true)?,
            &mut CommitMessageProvider::new()?,
        ],
    )?;
    Ok(description)
}

/// Options to use when executing a `RebasePlan`.
#[derive(Clone, Debug)]
pub struct ExecuteRebasePlanOptions {
    /// The time which should be recorded for this event.
    pub now: SystemTime,

    /// The transaction ID for this event.
    pub event_tx_id: EventTransactionId,

    /// If `true`, any rewritten commits will keep the same authored and
    /// committed timestamps. If `false`, the committed timestamps will be updated
    /// to the current time.
    pub preserve_timestamps: bool,

    /// Force an in-memory rebase (as opposed to an on-disk rebase).
    pub force_in_memory: bool,

    /// Force an on-disk rebase (as opposed to an in-memory rebase).
    pub force_on_disk: bool,
}

/// Execute the provided rebase plan. Returns the exit status (zero indicates
/// success).
pub fn execute_rebase_plan(
    glyphs: &Glyphs,
    git_run_info: &GitRunInfo,
    repo: &Repo,
    rebase_plan: &RebasePlan,
    options: &ExecuteRebasePlanOptions,
) -> anyhow::Result<isize> {
    let ExecuteRebasePlanOptions {
        now: _,
        event_tx_id: _,
        preserve_timestamps: _,
        force_in_memory,
        force_on_disk,
    } = options;

    if !force_on_disk {
        println!("Attempting rebase in-memory...");
        match rebase_in_memory(glyphs, &repo, &rebase_plan, &options)? {
            RebaseInMemoryResult::Succeeded { rewritten_oids } => {
                post_rebase_in_memory(git_run_info, repo, &rewritten_oids, &options)?;
                println!("In-memory rebase succeeded.");
                return Ok(0);
            }
            RebaseInMemoryResult::CannotRebaseMergeCommit { commit_oid } => {
                println!(
                    "Merge commits currently can't be rebased with `git move`. The merge commit was: {}",
                    printable_styled_string(glyphs, friendly_describe_commit(repo, commit_oid)?)?
                );
                return Ok(1);
            }
            RebaseInMemoryResult::MergeConflict { commit_oid } => {
                if *force_in_memory {
                    println!(
                        "Merge conflict. The conflicting commit was: {}",
                        printable_styled_string(
                            glyphs,
                            friendly_describe_commit(repo, commit_oid)?
                        )?,
                    );
                    println!("Aborting since an in-memory rebase was requested.");
                    return Ok(1);
                } else {
                    println!(
                        "Merge conflict, falling back to rebase on-disk. The conflicting commit was: {}",
                        printable_styled_string(glyphs, friendly_describe_commit(repo, commit_oid)?)?,
                    );
                }
            }
        }
    }

    if !force_in_memory {
        let result = rebase_on_disk(git_run_info, repo, &rebase_plan, &options)?;
        return Ok(result);
    }

    anyhow::bail!(
        "Both force_in_memory and force_on_disk were requested, but these options conflict"
    )
}

#[cfg(test)]
mod tests {
    use crate::core::eventlog::EventLogDb;
    use crate::core::graph::{make_graph, BranchOids, HeadOid, MainBranchOid};
    use crate::core::mergebase::MergeBaseDb;
    use crate::testing::{make_git, Git, GitRunOptions};
    use crate::util::{get_branch_oid_to_names, get_db_conn};

    use super::*;

    fn find_rewrite_target_helper(git: &Git, oid: git2::Oid) -> anyhow::Result<Option<git2::Oid>> {
        let repo = git.get_repo()?;
        let conn = get_db_conn(&repo)?;
        let merge_base_db = MergeBaseDb::new(&conn)?;
        let event_log_db = EventLogDb::new(&conn)?;
        let event_replayer = EventReplayer::from_event_log_db(&event_log_db)?;
        let event_cursor = event_replayer.make_default_cursor();
        let head_oid = repo.get_head_oid()?;
        let main_branch_oid = repo.get_main_branch_oid()?;
        let branch_oid_to_names = get_branch_oid_to_names(&repo)?;
        let graph = make_graph(
            &repo,
            &merge_base_db,
            &event_replayer,
            event_cursor,
            &HeadOid(head_oid),
            &MainBranchOid(main_branch_oid),
            &BranchOids(branch_oid_to_names.keys().copied().collect()),
            true,
        )?;

        let rewrite_target = find_rewrite_target(&graph, &event_replayer, event_cursor, oid);
        Ok(rewrite_target)
    }

    #[test]
    fn test_find_rewrite_target() -> anyhow::Result<()> {
        let git = make_git()?;

        git.init_repo()?;
        let commit_time = 1;
        let old_oid = git.commit_file("test1", commit_time)?;

        {
            git.run(&["commit", "--amend", "-m", "test1 amended once"])?;
            let new_oid: git2::Oid = {
                let (stdout, _stderr) = git.run(&["rev-parse", "HEAD"])?;
                stdout.trim().parse()?
            };
            let rewrite_target = find_rewrite_target_helper(&git, old_oid)?;
            assert_eq!(rewrite_target, Some(new_oid));
        }

        {
            git.run(&["commit", "--amend", "-m", "test1 amended twice"])?;
            let new_oid: git2::Oid = {
                let (stdout, _stderr) = git.run(&["rev-parse", "HEAD"])?;
                stdout.trim().parse()?
            };
            let rewrite_target = find_rewrite_target_helper(&git, old_oid)?;
            assert_eq!(rewrite_target, Some(new_oid));
        }

        {
            git.run_with_options(
                &["commit", "--amend", "-m", "create test1.txt"],
                &GitRunOptions {
                    time: commit_time,
                    ..Default::default()
                },
            )?;
            let rewrite_target = find_rewrite_target_helper(&git, old_oid)?;
            assert_eq!(rewrite_target, None);
        }

        Ok(())
    }
}
