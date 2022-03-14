//! Automatically collects information which may be relevant for a bug report.

use std::collections::HashSet;
use std::time::SystemTime;

use bugreport::bugreport;
use bugreport::collector::{CollectionError, Collector};
use bugreport::format::Markdown;
use bugreport::report::ReportEntry;
use itertools::Itertools;

use crate::commands::smartlog::{make_smartlog_graph, render_graph};
use crate::core::dag::Dag;
use crate::core::effects::Effects;
use crate::core::eventlog::{Event, EventCursor, EventLogDb, EventReplayer};
use crate::core::formatting::{printable_styled_string, Glyphs};
use crate::core::node_descriptors::{
    BranchesDescriptor, CommitMessageDescriptor, CommitOidDescriptor,
    DifferentialRevisionDescriptor, ObsolescenceExplanationDescriptor, Redactor,
    RelativeTimeDescriptor,
};
use crate::git::{GitRunInfo, Repo, RepoReferencesSnapshot, ResolvedReferenceInfo};

fn redact_event(redactor: &Redactor, event: &Event) -> String {
    let event = match event.clone() {
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
    };

    format!("{:?}", event)
}

fn describe_event_cursor(
    now: SystemTime,
    repo: &Repo,
    event_replayer: &EventReplayer,
    dag: &Dag,
    head_info: &ResolvedReferenceInfo,
    references_snapshot: &RepoReferencesSnapshot,
    redactor: &Redactor,
    event_cursor: EventCursor,
) -> eyre::Result<Vec<String>> {
    let event_description_lines = match event_replayer.get_tx_events_before_cursor(event_cursor) {
        Some((event_id, events)) => {
            let mut lines = vec![
                format!(
                    "##### Event ID: {}, transaction ID: {}",
                    event_id,
                    events[0].get_event_tx_id().to_string()
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
    let graph = make_smartlog_graph(&effects, repo, dag, event_replayer, event_cursor, true)?;
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
        .map(|line| printable_styled_string(&glyphs, line))
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
    let dag = Dag::open_and_sync(
        effects,
        &repo,
        &event_replayer,
        event_cursor,
        &references_snapshot,
    )?;

    let redactor = Redactor::new({
        let mut preserved_ref_names = HashSet::new();
        preserved_ref_names.insert(repo.get_main_branch_reference()?.get_name()?);
        preserved_ref_names
    });

    let mut event_text_lines = Vec::new();
    let num_events = 5;
    for i in 0..num_events {
        let event_cursor = event_replayer.advance_cursor_by_transaction(event_cursor, -i);
        let lines = describe_event_cursor(
            now,
            &repo,
            &event_replayer,
            &dag,
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

/// Generate information suitable for inclusion in a bug report.
pub fn bug_report(effects: &Effects, git_run_info: &GitRunInfo) -> eyre::Result<isize> {
    use bugreport::collector::*;
    bugreport!()
        .info(SoftwareVersion::default())
        .info(OperatingSystem::default())
        .info(CommandLine::default())
        .info(EnvironmentVariables::list(&["SHELL", "EDITOR"]))
        .info(CommandOutput::new("Git version", "git", &["version"]))
        .info(EventCollector {
            effects: effects.clone(),
            git_run_info: git_run_info.clone(),
        })
        .print::<Markdown>();

    Ok(0)
}
