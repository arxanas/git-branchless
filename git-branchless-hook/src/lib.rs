//! Callbacks for Git hooks.
//!
//! Git uses "hooks" to run user-defined scripts after certain events. We
//! extensively use these hooks to track user activity and e.g. decide if a
//! commit should be considered obsolete.
//!
//! The hooks are installed by the `branchless init` command. This module
//! contains the implementations for the hooks.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_if_conditions)]

use std::fmt::Write;
use std::fs::File;
use std::io::{stdin, BufRead};
use std::time::SystemTime;

use eyre::Context;
use git_branchless_invoke::CommandContext;
use git_branchless_opts::{HookArgs, HookSubcommand};
use itertools::Itertools;
use lib::core::dag::Dag;
use lib::core::repo_ext::RepoExt;
use lib::core::rewrite::rewrite_hooks::get_deferred_commits_path;
use lib::util::EyreExitOr;
use tracing::{error, instrument, warn};

use lib::core::eventlog::{should_ignore_ref_updates, Event, EventLogDb, EventReplayer};
use lib::core::formatting::{Glyphs, Pluralize};
use lib::core::gc::{gc, mark_commit_reachable};
use lib::git::{CategorizedReferenceName, MaybeZeroOid, NonZeroOid, ReferenceName, Repo};

use lib::core::effects::Effects;
pub use lib::core::rewrite::rewrite_hooks::{
    hook_drop_commit_if_empty, hook_post_rewrite, hook_register_extra_post_rewrite_hook,
    hook_skip_upstream_applied_commit,
};

/// Handle Git's `post-checkout` hook.
///
/// See the man-page for `githooks(5)`.
#[instrument]
fn hook_post_checkout(
    effects: &Effects,
    previous_head_oid: &str,
    current_head_oid: &str,
    is_branch_checkout: isize,
) -> eyre::Result<()> {
    if is_branch_checkout == 0 {
        return Ok(());
    }

    let now = SystemTime::now();
    let timestamp = now.duration_since(SystemTime::UNIX_EPOCH)?;
    writeln!(
        effects.get_output_stream(),
        "branchless: processing checkout"
    )?;

    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "hook-post-checkout")?;
    event_log_db.add_events(vec![Event::RefUpdateEvent {
        timestamp: timestamp.as_secs_f64(),
        event_tx_id,
        old_oid: previous_head_oid.parse()?,
        new_oid: {
            let oid: MaybeZeroOid = current_head_oid.parse()?;
            oid
        },
        ref_name: ReferenceName::from("HEAD"),
        message: None,
    }])?;
    Ok(())
}

fn hook_post_commit_common(effects: &Effects, hook_name: &str) -> eyre::Result<()> {
    let now = SystemTime::now();
    let glyphs = Glyphs::detect();
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;

    let commit_oid = match repo.get_head_info()?.oid {
        Some(commit_oid) => commit_oid,
        None => {
            // A strange situation, but technically possible.
            warn!(
                "`{}` hook called, but could not determine the OID of `HEAD`",
                hook_name
            );
            return Ok(());
        }
    };

    let commit = repo
        .find_commit_or_fail(commit_oid)
        .wrap_err("Looking up `HEAD` commit")?;
    mark_commit_reachable(&repo, commit_oid)
        .wrap_err("Marking commit as reachable for GC purposes")?;

    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let references_snapshot = repo.get_references_snapshot()?;
    Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    if repo.is_rebase_underway()? {
        let deferred_commits_path = get_deferred_commits_path(&repo);
        let mut deferred_commits_file = File::options()
            .create(true)
            .append(true)
            .open(&deferred_commits_path)
            .with_context(|| {
                format!("Opening deferred commits file at {deferred_commits_path:?}")
            })?;

        use std::io::Write;
        writeln!(deferred_commits_file, "{commit_oid}")?;
        return Ok(());
    }

    let timestamp = commit.get_time().to_system_time()?;

    // Potentially lossy conversion. The semantics are to round to the nearest
    // possible float:
    // https://doc.rust-lang.org/reference/expressions/operator-expr.html#semantics.
    // We don't rely on the timestamp's correctness for anything, so this is
    // okay.
    let timestamp = timestamp
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs_f64();

    let event_tx_id = event_log_db.make_transaction_id(now, hook_name)?;
    event_log_db.add_events(vec![Event::CommitEvent {
        timestamp,
        event_tx_id,
        commit_oid: commit.get_oid(),
    }])?;
    writeln!(
        effects.get_output_stream(),
        "branchless: processed commit: {}",
        glyphs.render(commit.friendly_describe(&glyphs)?)?,
    )?;

    Ok(())
}

/// Handle Git's `post-commit` hook.
///
/// See the man-page for `githooks(5)`.
#[instrument]
fn hook_post_commit(effects: &Effects) -> eyre::Result<()> {
    hook_post_commit_common(effects, "post-commit")
}

/// Handle Git's `post-merge` hook. It seems that Git doesn't invoke the
/// `post-commit` hook after a merge commit, so we need to handle this case
/// explicitly with another hook.
///
/// See the man-page for `githooks(5)`.
#[instrument]
fn hook_post_merge(effects: &Effects, _is_squash_merge: isize) -> eyre::Result<()> {
    hook_post_commit_common(effects, "post-merge")
}

/// Handle Git's `post-applypatch` hook.
///
/// See the man-page for `githooks(5)`.
#[instrument]
fn hook_post_applypatch(effects: &Effects) -> eyre::Result<()> {
    hook_post_commit_common(effects, "post-applypatch")
}

mod reference_transaction {
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use std::str::FromStr;

    use eyre::Context;
    use itertools::Itertools;
    use lazy_static::lazy_static;
    use tracing::{instrument, warn};

    use lib::git::{MaybeZeroOid, ReferenceName, Repo};

    #[instrument]
    fn parse_packed_refs_line(line: &str) -> Option<(ReferenceName, MaybeZeroOid)> {
        if line.is_empty() {
            return None;
        }
        if line.starts_with('#') {
            // The leading `# pack-refs with:` pragma.
            return None;
        }
        if !line.starts_with(|c: char| c.is_ascii_hexdigit()) {
            // The leading `# pack-refs with:` pragma.
            warn!(?line, "Unrecognized pack-refs line starting character");
            return None;
        }

        lazy_static! {
            static ref RE: regex::Regex = regex::Regex::new(r"^([^ ]+) (.+)$").unwrap();
        };
        match RE.captures(line) {
            None => {
                warn!(?line, "No regex match for pack-refs line");
                None
            }

            Some(captures) => {
                let oid = &captures[1];
                let oid = match MaybeZeroOid::from_str(oid) {
                    Ok(oid) => oid,
                    Err(err) => {
                        warn!(?oid, ?err, "Could not parse OID for pack-refs line");
                        return None;
                    }
                };

                let reference_name = &captures[2];
                let reference_name = ReferenceName::from(reference_name);

                Some((reference_name, oid))
            }
        }
    }

    #[cfg(test)]
    #[test]
    fn test_parse_packed_refs_line() {
        use super::*;

        let line = "1234567812345678123456781234567812345678 refs/foo/bar";
        let name = ReferenceName::from("refs/foo/bar");
        let oid = MaybeZeroOid::from_str("1234567812345678123456781234567812345678").unwrap();
        assert_eq!(parse_packed_refs_line(line), Some((name, oid)));
    }

    #[instrument]
    pub fn read_packed_refs_file(
        repo: &Repo,
    ) -> eyre::Result<HashMap<ReferenceName, MaybeZeroOid>> {
        let packed_refs_file_path = repo.get_packed_refs_path();
        let file = match File::open(packed_refs_file_path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(err) => return Err(err.into()),
        };

        let reader = BufReader::new(file);
        let mut result = HashMap::new();
        for line in reader.lines() {
            let line = line.wrap_err("Reading line from packed-refs")?;
            if line.is_empty() {
                continue;
            }
            if let Some((k, v)) = parse_packed_refs_line(&line) {
                result.insert(k, v);
            }
        }
        Ok(result)
    }

    #[derive(Debug, PartialEq, Eq)]
    pub struct ParsedReferenceTransactionLine {
        pub ref_name: ReferenceName,
        pub old_oid: MaybeZeroOid,
        pub new_oid: MaybeZeroOid,
    }

    #[instrument]
    pub fn parse_reference_transaction_line(
        line: &str,
    ) -> eyre::Result<ParsedReferenceTransactionLine> {
        let fields = line.split(' ').collect_vec();
        match fields.as_slice() {
            [old_value, new_value, ref_name] => Ok(ParsedReferenceTransactionLine {
                ref_name: ReferenceName::from(*ref_name),
                old_oid: MaybeZeroOid::from_str(old_value)?,
                new_oid: MaybeZeroOid::from_str(new_value)?,
            }),
            _ => {
                eyre::bail!(
                    "Unexpected number of fields in reference-transaction line: {:?}",
                    &line
                )
            }
        }
    }

    #[cfg(test)]
    #[test]
    fn test_parse_reference_transaction_line() -> eyre::Result<()> {
        use lib::core::eventlog::should_ignore_ref_updates;

        let line = "123abc 456def refs/heads/mybranch";
        assert_eq!(
            parse_reference_transaction_line(line)?,
            ParsedReferenceTransactionLine {
                old_oid: "123abc".parse()?,
                new_oid: {
                    let oid: MaybeZeroOid = "456def".parse()?;
                    oid
                },
                ref_name: ReferenceName::from("refs/heads/mybranch"),
            }
        );

        {
            let line = "123abc 456def ORIG_HEAD";
            let parsed_line = parse_reference_transaction_line(line)?;
            assert_eq!(
                parsed_line,
                ParsedReferenceTransactionLine {
                    old_oid: "123abc".parse()?,
                    new_oid: "456def".parse()?,
                    ref_name: ReferenceName::from("ORIG_HEAD"),
                }
            );
            assert!(should_ignore_ref_updates(&parsed_line.ref_name));
        }

        let line = "there are not three fields here";
        assert!(parse_reference_transaction_line(line).is_err());

        Ok(())
    }

    /// As per the discussion at
    /// https://public-inbox.org/git/CAKjfCeBcuYC3OXRVtxxDGWRGOxC38Fb7CNuSh_dMmxpGVip_9Q@mail.gmail.com/,
    /// the OIDs passed to the reference transaction can't actually be trusted
    /// when dealing with packed references, so we need to look up their actual
    /// values on disk again. See https://git-scm.com/docs/git-pack-refs for
    /// details about packed references.
    ///
    /// Supposing we have a ref named `refs/heads/foo` pointing to an OID
    /// `abc123`, when references are packed, we'll first see a transaction like
    /// this:
    ///
    /// ```text
    /// 000000 abc123 refs/heads/foo
    /// ```
    ///
    /// And immediately afterwards see a transaction like this:
    ///
    /// ```text
    /// abc123 000000 refs/heads/foo
    /// ```
    ///
    /// If considered naively, this would suggest that the reference was created
    /// (even though it already exists!) and then deleted (even though it still
    /// exists!).
    #[instrument]
    pub fn fix_packed_reference_oid(
        repo: &Repo,
        packed_references: &HashMap<ReferenceName, MaybeZeroOid>,
        parsed_line: ParsedReferenceTransactionLine,
    ) -> ParsedReferenceTransactionLine {
        match parsed_line {
            ParsedReferenceTransactionLine {
                ref_name,
                old_oid: MaybeZeroOid::Zero,
                new_oid,
            } if packed_references.get(&ref_name) == Some(&new_oid) => {
                // The reference claims to have been created, but it appears to
                // already be in the `packed-refs` file with that OID. Most
                // likely it was being packed in this operation.
                ParsedReferenceTransactionLine {
                    ref_name,
                    old_oid: new_oid,
                    new_oid,
                }
            }

            ParsedReferenceTransactionLine {
                ref_name,
                old_oid,
                new_oid: MaybeZeroOid::Zero,
            } if packed_references.get(&ref_name) == Some(&old_oid) => {
                // The reference claims to have been deleted, but it's still in
                // the `packed-refs` file with that OID. Most likely it was
                // being packed in this operation.
                ParsedReferenceTransactionLine {
                    ref_name,
                    old_oid,
                    new_oid: old_oid,
                }
            }

            other => other,
        }
    }
}

/// Handle Git's `reference-transaction` hook.
///
/// See the man-page for `githooks(5)`.
#[instrument]
fn hook_reference_transaction(effects: &Effects, transaction_state: &str) -> eyre::Result<()> {
    use reference_transaction::{
        fix_packed_reference_oid, parse_reference_transaction_line, read_packed_refs_file,
        ParsedReferenceTransactionLine,
    };

    if transaction_state != "committed" {
        return Ok(());
    }
    let now = SystemTime::now();

    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_tx_id = event_log_db.make_transaction_id(now, "reference-transaction")?;

    let packed_references = read_packed_refs_file(&repo)?;

    let parsed_lines: Vec<ParsedReferenceTransactionLine> = stdin()
        .lock()
        .split(b'\n')
        .filter_map(|line| {
            let line = match line {
                Ok(line) => line,
                Err(_) => return None,
            };
            let line = match std::str::from_utf8(&line) {
                Ok(line) => line,
                Err(err) => {
                    error!(?err, ?line, "Could not parse reference-transaction line");
                    return None;
                }
            };
            match parse_reference_transaction_line(line) {
                Ok(line) => Some(line),
                Err(err) => {
                    error!(?err, ?line, "Could not parse reference-transaction-line");
                    None
                }
            }
        })
        .filter(
            |ParsedReferenceTransactionLine {
                 ref_name,
                 old_oid: _,
                 new_oid: _,
             }| {
                !should_ignore_ref_updates(ref_name)
                    && match CategorizedReferenceName::new(ref_name) {
                        CategorizedReferenceName::RemoteBranch { .. } => false,
                        CategorizedReferenceName::OtherRef { .. } => false,
                        CategorizedReferenceName::LocalBranch { .. } => true,
                    }
            },
        )
        .map(|parsed_line| fix_packed_reference_oid(&repo, &packed_references, parsed_line))
        .collect();
    if parsed_lines.is_empty() {
        return Ok(());
    }

    let num_reference_updates = Pluralize {
        determiner: None,
        amount: parsed_lines.len(),
        unit: ("update", "updates"),
    };
    writeln!(
        effects.get_output_stream(),
        "branchless: processing {}: {}",
        num_reference_updates,
        parsed_lines
            .iter()
            .map(
                |ParsedReferenceTransactionLine {
                     ref_name,
                     old_oid: _,
                     new_oid: _,
                 }| { CategorizedReferenceName::new(ref_name).friendly_describe() }
            )
            .map(|description| format!("{}", console::style(description).green()))
            .sorted()
            .collect::<Vec<_>>()
            .join(", ")
    )?;

    let timestamp = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .wrap_err("Calculating timestamp")?
        .as_secs_f64();
    let events = parsed_lines
        .into_iter()
        .map(
            |ParsedReferenceTransactionLine {
                 ref_name,
                 old_oid,
                 new_oid,
             }| {
                Event::RefUpdateEvent {
                    timestamp,
                    event_tx_id,
                    ref_name,
                    old_oid,
                    new_oid,
                    message: None,
                }
            },
        )
        .collect::<Vec<Event>>();
    event_log_db.add_events(events)?;

    Ok(())
}

/// `hook` subcommand.
#[instrument]
pub fn command_main(ctx: CommandContext, args: HookArgs) -> EyreExitOr<()> {
    let CommandContext {
        effects,
        git_run_info,
    } = ctx;
    let HookArgs { subcommand } = args;

    match subcommand {
        HookSubcommand::DetectEmptyCommit { old_commit_oid } => {
            let old_commit_oid: NonZeroOid = old_commit_oid.parse()?;
            hook_drop_commit_if_empty(&effects, old_commit_oid)?;
        }

        HookSubcommand::PreAutoGc => {
            gc(&effects)?;
        }

        HookSubcommand::PostApplypatch => {
            hook_post_applypatch(&effects)?;
        }

        HookSubcommand::PostCheckout {
            previous_commit,
            current_commit,
            is_branch_checkout,
        } => {
            hook_post_checkout(
                &effects,
                &previous_commit,
                &current_commit,
                is_branch_checkout,
            )?;
        }

        HookSubcommand::PostCommit => {
            hook_post_commit(&effects)?;
        }

        HookSubcommand::PostMerge { is_squash_merge } => {
            hook_post_merge(&effects, is_squash_merge)?;
        }

        HookSubcommand::PostRewrite { rewrite_type } => {
            hook_post_rewrite(&effects, &git_run_info, &rewrite_type)?;
        }

        HookSubcommand::ReferenceTransaction { transaction_state } => {
            hook_reference_transaction(&effects, &transaction_state)?;
        }

        HookSubcommand::RegisterExtraPostRewriteHook => {
            hook_register_extra_post_rewrite_hook()?;
        }

        HookSubcommand::SkipUpstreamAppliedCommit { commit_oid } => {
            let commit_oid: NonZeroOid = commit_oid.parse()?;
            hook_skip_upstream_applied_commit(&effects, commit_oid)?;
        }
    }

    Ok(Ok(()))
}
