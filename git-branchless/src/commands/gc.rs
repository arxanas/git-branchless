//! Invokes git-branchless's garbage-collection mechanisms.

use std::fmt::Write;

use lib::core::gc::find_dangling_references;
use tracing::instrument;

use lib::core::effects::Effects;
use lib::core::eventlog::{EventLogDb, EventReplayer};
use lib::core::formatting::Pluralize;
use lib::git::Repo;

/// Run branchless's garbage collection.
///
/// Frees any references to commits which are no longer visible in the smartlog.
#[instrument]
pub fn gc(effects: &Effects) -> eyre::Result<()> {
    let repo = Repo::from_current_dir()?;
    let conn = repo.get_db_conn()?;
    let event_log_db = EventLogDb::new(&conn)?;
    let event_replayer = EventReplayer::from_event_log_db(effects, &repo, &event_log_db)?;
    let event_cursor = event_replayer.make_default_cursor();

    writeln!(
        effects.get_output_stream(),
        "branchless: collecting garbage"
    )?;
    let dangling_references = find_dangling_references(&repo, &event_replayer, event_cursor)?;
    let num_dangling_references = Pluralize {
        determiner: None,
        amount: dangling_references.len(),
        unit: ("dangling reference", "dangling references"),
    }
    .to_string();
    for mut reference in dangling_references.into_iter() {
        reference.delete()?;
    }

    writeln!(
        effects.get_output_stream(),
        "branchless: {num_dangling_references} deleted",
    )?;
    Ok(())
}
