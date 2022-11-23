//! Automatically collects information which may be relevant for a bug report.

use std::collections::HashSet;
use std::time::SystemTime;

use bugreport::bugreport;
use bugreport::collector::{CollectionError, Collector};
use bugreport::format::Markdown;
use bugreport::report::ReportEntry;
use itertools::Itertools;
use lib::core::repo_ext::{RepoExt, RepoReferencesSnapshot};
use lib::util::ExitCode;

use git_branchless_revset::resolve_default_smartlog_commits;
use git_branchless_smartlog::{make_smartlog_graph, render_graph};
use lib::core::dag::Dag;
use lib::core::effects::Effects;
use lib::core::eventlog::{Event, EventCursor, EventLogDb, EventReplayer};
use lib::core::formatting::Glyphs;
use lib::core::node_descriptors::{
    BranchesDescriptor, CommitMessageDescriptor, CommitOidDescriptor,
    DifferentialRevisionDescriptor, ObsolescenceExplanationDescriptor, Redactor,
    RelativeTimeDescriptor,
};
use lib::git::{GitRunInfo, Repo, ResolvedReferenceInfo};

use super::init::{determine_hook_path, Hook, ALL_HOOKS};

fn redact_event(redactor: &Redactor, event: &Event) -> String {
    let event = match event.clone() {
        // Explicitly list all variants and fields here so we're forced to audit it if we add any.
        event @ (Event::RewriteEvent {
            timestamp: _,
            event_tx_id: _,
            old_commit_oid: _,
            new_commit_oid: _,
        }
        | Event::CommitEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid: _,
        }
        | Event::ObsoleteEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid: _,
        }
        | Event::UnobsoleteEvent {
            timestamp: _,
            event_tx_id: _,
            commit_oid: _,
        }) => event,

        Event::RefUpdateEvent {
            timestamp,
            event_tx_id,
            ref_name,
            old_oid,
            new_oid,
            message,
        } => {
            let ref_name = redactor.redact_ref_name(ref_name);
            Event::RefUpdateEvent {
                timestamp,
                event_tx_id,
                ref_name,
                old_oid,
                new_oid,
                message,
            }
        }

        Event::WorkingCopySnapshot {
            timestamp,
            event_tx_id,
            head_oid,
            commit_oid,
            ref_name,
        } => {
            let ref_name = ref_name.map(|name| redactor.redact_ref_name(name));
            Event::WorkingCopySnapshot {
                timestamp,
                event_tx_id,
                head_oid,
                commit_oid,
                ref_name,
            }
        }
    };

    format!("{:?}", event)
}

fn describe_event_cursor(
    now: SystemTime,
    repo: &Repo,
    event_log_db: &EventLogDb,
    event_replayer: &EventReplayer,
    dag: &mut Dag,
    head_info: &ResolvedReferenceInfo,
    references_snapshot: &RepoReferencesSnapshot,
    redactor: &Redactor,
    event_cursor: EventCursor,
) -> eyre::Result<Vec<String>> {
    let event_description_lines = match event_replayer.get_tx_events_before_cursor(event_cursor) {
        Some((event_id, events)) => {
            let event_tx_id = events[0].get_event_tx_id();
            let transaction_message = event_log_db
                .get_transaction_message(event_tx_id)
                .unwrap_or_else(|_err| "<failed to query>".to_string());
            let mut lines = vec![
                format!(
                    "##### Event ID: {}, transaction ID: {} (message: {transaction_message})",
                    event_id,
                    event_tx_id.to_string()
                ),
                "".to_string(),
            ];
            lines.extend(
                events
                    .iter()
                    .map(|event| format!("1. `{}`", redact_event(redactor, event))),
            );
            lines
        }
        None => {
            let lines = vec!["There are no previous available events.".to_string()];
            lines
        }
    };

    let glyphs = Glyphs::text();
    let effects = Effects::new(glyphs.clone());
    let commits = resolve_default_smartlog_commits(&effects, repo, dag)?;
    let graph = make_smartlog_graph(&effects, repo, dag, event_replayer, event_cursor, &commits)?;
    let graph_lines = render_graph(
        &effects,
        repo,
        dag,
        &graph,
        references_snapshot.head_oid,
        &mut [
            &mut CommitOidDescriptor::new(true)?,
            &mut RelativeTimeDescriptor::new(repo, now)?,
            &mut ObsolescenceExplanationDescriptor::new(event_replayer, event_cursor)?,
            &mut BranchesDescriptor::new(repo, head_info, references_snapshot, redactor)?,
            &mut DifferentialRevisionDescriptor::new(repo, redactor)?,
            &mut CommitMessageDescriptor::new(redactor)?,
        ],
    )?;
    let graph_lines = graph_lines
        .into_iter()
        .map(|line| glyphs.render(line))
        .try_collect()?;

    Ok([
        event_description_lines,
        vec!["```".to_string()],
        graph_lines,
        vec!["```".to_string()],
    ]
    .concat())
}

fn collect_events(effects: &Effects, git_run_info: &GitRunInfo) -> eyre::Result<ReportEntry> {
    let now = SystemTime::now();
    let repo = Repo::from_dir(&git_run_info.working_directory)?;
    let head_info = repo.get_head_info()?;
    let references_snapshot = repo.get_references_snapshot()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();
    let mut dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let redactor = Redactor::new({
        let mut preserved_ref_names = HashSet::new();
        preserved_ref_names.insert(repo.get_main_branch()?.get_reference_name()?);
        preserved_ref_names
    });

    let mut event_text_lines = Vec::new();
    let num_events = 5;
    for i in 0..num_events {
        let event_cursor = event_replayer.advance_cursor_by_transaction(event_cursor, -i);
        let lines = describe_event_cursor(
            now,
            &repo,
            &event_log_db,
            &event_replayer,
            &mut dag,
            &head_info,
            &references_snapshot,
            &redactor,
            event_cursor,
        )?;

        event_text_lines.extend(lines);
    }
    Ok(ReportEntry::Text(format!(
        "
<details>
<summary>Show {} events</summary>

{}

</details>",
        num_events,
        event_text_lines.join("\n")
    )))
}

struct EventCollector {
    effects: Effects,
    git_run_info: GitRunInfo,
}

impl Collector for EventCollector {
    fn description(&self) -> &str {
        "Events"
    }

    fn collect(
        &mut self,
        _crate_info: &bugreport::CrateInfo,
    ) -> Result<ReportEntry, CollectionError> {
        collect_events(&self.effects, &self.git_run_info)
            .map_err(|e| CollectionError::CouldNotRetrieve(format!("Error: {}", e)))
    }
}

struct HookCollector;

fn collect_hooks() -> eyre::Result<ReportEntry> {
    let repo = Repo::from_current_dir()?;
    let hook_contents = {
        let mut result = Vec::new();
        for (hook_type, _content) in ALL_HOOKS {
            let hook_path = match determine_hook_path(&repo, hook_type)? {
                Hook::RegularHook { path } | Hook::MultiHook { path } => path,
            };
            let hook_contents = match std::fs::read_to_string(hook_path) {
                Ok(hook_contents) => hook_contents,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => "<not found>".to_string(),
                Err(err) => return Err(err.into()),
            };
            result.push((hook_type, hook_contents));
        }
        result
    };
    let num_hooks = hook_contents.len();
    let hook_lines = {
        let mut result = Vec::new();
        for (hook_type, hook_contents) in hook_contents {
            result.push(format!("##### Hook `{hook_type}`"));
            result.push("".to_string());
            result.push("```".to_string());

            match hook_contents.strip_suffix('\n') {
                Some(hook_contents) => {
                    result.push(hook_contents.to_string());
                }
                None => {
                    result.push(hook_contents);
                    result.push("<missing newline>".to_string());
                }
            }

            result.push("```".to_string());
        }
        result
    };

    Ok(ReportEntry::Text(format!(
        "
<details>
<summary>Show {} hooks</summary>

{}

</details>",
        num_hooks,
        hook_lines.join("\n")
    )))
}

impl Collector for HookCollector {
    fn description(&self) -> &str {
        "Hooks"
    }

    fn collect(
        &mut self,
        _crate_info: &bugreport::CrateInfo,
    ) -> Result<ReportEntry, CollectionError> {
        collect_hooks().map_err(|e| CollectionError::CouldNotRetrieve(format!("Error: {}", e)))
    }
}

/// Generate information suitable for inclusion in a bug report.
pub fn bug_report(effects: &Effects, git_run_info: &GitRunInfo) -> eyre::Result<ExitCode> {
    use bugreport::collector::*;
    bugreport!()
        .info(SoftwareVersion::default())
        .info(OperatingSystem::default())
        .info(CommandLine::default())
        .info(EnvironmentVariables::list(&["SHELL", "EDITOR"]))
        .info(CommandOutput::new("Git version", "git", &["version"]))
        .info(HookCollector)
        .info(EventCollector {
            effects: effects.clone(),
            git_run_info: git_run_info.clone(),
        })
        .print::<Markdown>();

    Ok(ExitCode(0))
}
