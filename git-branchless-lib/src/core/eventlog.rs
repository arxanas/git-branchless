//! Process our event log.
//!
//! We use Git hooks to record the actions that the user takes over time, and put
//! them in persistent storage. Later, we play back the actions in order to
//! determine what actions the user took on the repository, and which commits
//! they're still working on.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::convert::{TryFrom, TryInto};

use std::str::FromStr;
use std::time::{Duration, SystemTime};

use eyre::Context;
use tracing::{error, instrument};

use crate::core::effects::{Effects, OperationType};
use crate::core::repo_ext::RepoExt;
use crate::git::{CategorizedReferenceName, MaybeZeroOid, NonZeroOid, ReferenceName, Repo};

use super::repo_ext::RepoReferencesSnapshot;

/// When this environment variable is set, we reuse the ID for the transaction
/// which the caller has already started.
pub const BRANCHLESS_TRANSACTION_ID_ENV_VAR: &str = "BRANCHLESS_TRANSACTION_ID";

// Wrapper around the row stored directly in the database.
#[derive(Clone, Debug)]
struct Row {
    timestamp: f64,
    type_: String,
    event_tx_id: isize,
    ref1: Option<ReferenceName>,
    ref2: Option<ReferenceName>,
    ref_name: Option<ReferenceName>,
    message: Option<ReferenceName>,
}

/// The ID associated with the transactions that created an event.
///
/// A "event transaction" is a group of logically-related events. For example,
/// during a rebase operation, all of the rebased commits have different
/// `CommitEvent`s, but should belong to the same transaction. This improves the
/// experience for `git undo`, since the user probably wants to see or undo all
/// of the logically-related events at once, rather than individually.
///
/// Note that some logically-related events may not be included together in the
/// same transaction. For example, if a rebase is interrupted due to a merge
/// conflict, then the commits applied due to `git rebase` and the commits
/// applied due to `git rebase --continue` may not share the same transaction.
/// In this sense, a transaction is "best-effort".
///
/// Unlike in a database, there is no specific guarantee that an event
/// transaction is an atomic unit of work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventTransactionId {
    /// A normal transaction ID.
    Id(isize),

    /// A value indicating that the no events should actually be added for this transaction.
    Suppressed,
}

impl ToString for EventTransactionId {
    fn to_string(&self) -> String {
        match self {
            EventTransactionId::Id(event_id) => event_id.to_string(),
            EventTransactionId::Suppressed => "SUPPRESSED".to_string(),
        }
    }
}

impl FromStr for EventTransactionId {
    type Err = <isize as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "SUPPRESSED" {
            Ok(EventTransactionId::Suppressed)
        } else {
            let event_id = s.parse()?;
            Ok(EventTransactionId::Id(event_id))
        }
    }
}

/// An event that occurred to one of the commits in the repository.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Indicates that the commit was rewritten.
    ///
    /// Examples of rewriting include rebases and amended commits.
    ///
    /// We typically want to mark the new version of the commit as active and
    /// the old version of the commit as obsolete.
    RewriteEvent {
        /// The timestamp of the event.
        timestamp: f64,

        /// The transaction ID of the event.
        event_tx_id: EventTransactionId,

        /// The OID of the commit before the rewrite.
        old_commit_oid: MaybeZeroOid,

        /// The OID of the commit after the rewrite.
        new_commit_oid: MaybeZeroOid,
    },

    /// Indicates that a reference was updated.
    ///
    /// The most important reference we track is HEAD. In principle, we can also
    /// track branch moves in this way, but Git doesn't support the appropriate
    /// hook until v2.29 (`reference-transaction`).
    RefUpdateEvent {
        /// The timestamp of the event.
        timestamp: f64,

        /// The transaction ID of the event.
        event_tx_id: EventTransactionId,

        /// The full name of the reference that was updated.
        ///
        /// For example, `HEAD` or `refs/heads/master`.
        ref_name: ReferenceName,

        /// The old referent OID.
        old_oid: MaybeZeroOid,

        /// The updated referent OID.
        new_oid: MaybeZeroOid,

        /// A message associated with the rewrite, if any.
        message: Option<ReferenceName>,
    },

    /// Indicate that the user made a commit.
    ///
    /// User commits should be marked as active.
    CommitEvent {
        /// The timestamp of the event.
        timestamp: f64,

        /// The transaction ID of the event.
        event_tx_id: EventTransactionId,

        /// The new commit OID.
        commit_oid: NonZeroOid,
    },

    /// Indicates that a commit was explicitly obsoleted by the user.
    ///
    /// If the commit in question was not already active, then this has no
    /// practical effect.
    ObsoleteEvent {
        /// The timestamp of the event.
        timestamp: f64,

        /// The transaction ID of the event.
        event_tx_id: EventTransactionId,

        /// The OID of the commit that was obsoleted.
        commit_oid: NonZeroOid,
    },

    /// Indicates that a commit was explicitly un-obsoleted by the user.
    ///
    /// If the commit in question was not already obsolete, then this has no
    /// practical effect.
    UnobsoleteEvent {
        /// The timestamp of the event.
        timestamp: f64,

        /// The transaction ID of the event.
        event_tx_id: EventTransactionId,

        /// The OID of the commit that was unobsoleted.
        commit_oid: NonZeroOid,
    },

    /// Represents a snapshot of the working copy made at a certain time,
    /// typically before a potentially-destructive operation.
    WorkingCopySnapshot {
        /// The timestamp of the event.
        timestamp: f64,

        /// The transaction ID of the event.
        event_tx_id: EventTransactionId,

        /// The OID of the current HEAD commit.
        head_oid: MaybeZeroOid,

        /// The OID of the commit containing metadata about the working copy
        /// snapshot.
        commit_oid: NonZeroOid,

        /// The name of the checked-out branch, if any. This should be a full
        /// reference name like `refs/heads/foo`.
        ref_name: Option<ReferenceName>,
    },
}

impl Event {
    /// Get the timestamp associated with this event.
    pub fn get_timestamp(&self) -> SystemTime {
        let timestamp = match self {
            Event::RewriteEvent { timestamp, .. } => timestamp,
            Event::RefUpdateEvent { timestamp, .. } => timestamp,
            Event::CommitEvent { timestamp, .. } => timestamp,
            Event::ObsoleteEvent { timestamp, .. } => timestamp,
            Event::UnobsoleteEvent { timestamp, .. } => timestamp,
            Event::WorkingCopySnapshot { timestamp, .. } => timestamp,
        };
        SystemTime::UNIX_EPOCH + Duration::from_secs_f64(*timestamp)
    }

    /// Get the event transaction ID associated with this event.
    pub fn get_event_tx_id(&self) -> EventTransactionId {
        match self {
            Event::RewriteEvent { event_tx_id, .. } => *event_tx_id,
            Event::RefUpdateEvent { event_tx_id, .. } => *event_tx_id,
            Event::CommitEvent { event_tx_id, .. } => *event_tx_id,
            Event::ObsoleteEvent { event_tx_id, .. } => *event_tx_id,
            Event::UnobsoleteEvent { event_tx_id, .. } => *event_tx_id,
            Event::WorkingCopySnapshot { event_tx_id, .. } => *event_tx_id,
        }
    }
}

impl TryFrom<Event> for Row {
    type Error = ();

    fn try_from(event: Event) -> Result<Self, Self::Error> {
        let row = match event {
            Event::RewriteEvent {
                event_tx_id: EventTransactionId::Suppressed,
                ..
            }
            | Event::RefUpdateEvent {
                event_tx_id: EventTransactionId::Suppressed,
                ..
            }
            | Event::CommitEvent {
                event_tx_id: EventTransactionId::Suppressed,
                ..
            }
            | Event::ObsoleteEvent {
                event_tx_id: EventTransactionId::Suppressed,
                ..
            }
            | Event::UnobsoleteEvent {
                event_tx_id: EventTransactionId::Suppressed,
                ..
            }
            | Event::WorkingCopySnapshot {
                event_tx_id: EventTransactionId::Suppressed,
                ..
            } => return Err(()),

            Event::RewriteEvent {
                timestamp,
                event_tx_id: EventTransactionId::Id(event_tx_id),
                old_commit_oid,
                new_commit_oid,
            } => Row {
                timestamp,
                event_tx_id,
                type_: String::from("rewrite"),
                ref1: Some(old_commit_oid.into()),
                ref2: Some(new_commit_oid.into()),
                ref_name: None,
                message: None,
            },

            Event::RefUpdateEvent {
                timestamp,
                event_tx_id: EventTransactionId::Id(event_tx_id),
                ref_name,
                old_oid,
                new_oid,
                message,
            } => Row {
                timestamp,
                event_tx_id,
                type_: String::from("ref-move"),
                ref1: Some(old_oid.into()),
                ref2: Some(new_oid.into()),
                ref_name: Some(ref_name),
                message,
            },

            Event::CommitEvent {
                timestamp,
                event_tx_id: EventTransactionId::Id(event_tx_id),
                commit_oid,
            } => Row {
                timestamp,
                event_tx_id,
                type_: String::from("commit"),
                ref1: Some(commit_oid.into()),
                ref2: None,
                ref_name: None,
                message: None,
            },

            Event::ObsoleteEvent {
                timestamp,
                event_tx_id: EventTransactionId::Id(event_tx_id),
                commit_oid,
            } => Row {
                timestamp,
                event_tx_id,
                type_: String::from("hide"), // historical name
                ref1: Some(commit_oid.into()),
                ref2: None,
                ref_name: None,
                message: None,
            },

            Event::UnobsoleteEvent {
                timestamp,
                event_tx_id: EventTransactionId::Id(event_tx_id),
                commit_oid,
            } => Row {
                timestamp,
                event_tx_id,
                type_: String::from("unhide"), // historical name
                ref1: Some(commit_oid.into()),
                ref2: None,
                ref_name: None,
                message: None,
            },

            Event::WorkingCopySnapshot {
                timestamp,
                event_tx_id: EventTransactionId::Id(event_tx_id),
                head_oid,
                commit_oid,
                ref_name,
            } => Row {
                timestamp,
                event_tx_id,
                type_: String::from("snapshot"),
                ref1: Some(head_oid.to_string().into()),
                ref2: Some(commit_oid.into()),
                ref_name,
                message: None,
            },
        };
        Ok(row)
    }
}

fn try_from_row_helper(row: &Row) -> Result<Event, eyre::Error> {
    let row: Row = row.clone();
    let Row {
        timestamp,
        event_tx_id,
        type_,
        ref_name,
        ref1,
        ref2,
        message,
    } = row;
    let event_tx_id = EventTransactionId::Id(event_tx_id);

    let get_oid =
        |reference_name: &Option<ReferenceName>, oid_name: &str| -> eyre::Result<MaybeZeroOid> {
            match reference_name {
                Some(reference_name) => {
                    let oid: MaybeZeroOid = reference_name.as_str().parse()?;
                    Ok(oid)
                }
                None => Err(eyre::eyre!(
                    "OID '{}' was `None` for event type '{}'",
                    oid_name,
                    type_
                )),
            }
        };

    let event = match type_.as_str() {
        "rewrite" => {
            let old_commit_oid = get_oid(&ref1, "old commit OID")?;
            let new_commit_oid = get_oid(&ref2, "new commit OID")?;
            Event::RewriteEvent {
                timestamp,
                event_tx_id,
                old_commit_oid,
                new_commit_oid,
            }
        }

        "ref-move" => {
            let ref_name =
                ref_name.ok_or_else(|| eyre::eyre!("ref-move event missing ref name"))?;
            let ref1 = ref1.ok_or_else(|| eyre::eyre!("ref-move event missing ref1"))?;
            let ref2 = ref2.ok_or_else(|| eyre::eyre!("ref-move event missing ref2"))?;
            Event::RefUpdateEvent {
                timestamp,
                event_tx_id,
                ref_name,
                old_oid: ref1.as_str().parse()?,
                new_oid: ref2.as_str().parse()?,
                message,
            }
        }

        "commit" => {
            let commit_oid: NonZeroOid = get_oid(&ref1, "commit OID")?.try_into()?;
            Event::CommitEvent {
                timestamp,
                event_tx_id,
                commit_oid,
            }
        }

        "hide" => {
            let commit_oid: NonZeroOid = get_oid(&ref1, "commit OID")?.try_into()?;
            Event::ObsoleteEvent {
                timestamp,
                event_tx_id,
                commit_oid,
            }
        }

        "unhide" => {
            let commit_oid: NonZeroOid = get_oid(&ref1, "commit OID")?.try_into()?;
            Event::UnobsoleteEvent {
                timestamp,
                event_tx_id,
                commit_oid,
            }
        }

        "snapshot" => {
            let head_oid: MaybeZeroOid = get_oid(&ref1, "head OID")?;
            let commit_oid: NonZeroOid = get_oid(&ref2, "commit OID")?.try_into()?;
            Event::WorkingCopySnapshot {
                timestamp,
                event_tx_id,
                head_oid,
                commit_oid,
                ref_name,
            }
        }

        other => eyre::bail!("Unknown event type {}", other),
    };
    Ok(event)
}

impl TryFrom<Row> for Event {
    type Error = eyre::Error;

    fn try_from(row: Row) -> Result<Self, Self::Error> {
        match try_from_row_helper(&row) {
            Ok(event) => Ok(event),
            Err(err) => {
                error!(?row, "Could not convert row into event");
                Err(err)
            }
        }
    }
}

/// Stores `Event`s on disk.
pub struct EventLogDb<'conn> {
    conn: &'conn rusqlite::Connection,
}

impl std::fmt::Debug for EventLogDb<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<EventLogDb path={:?}>", self.conn.path())
    }
}

#[instrument]
fn init_tables(conn: &rusqlite::Connection) -> eyre::Result<()> {
    conn.execute(
        "
CREATE TABLE IF NOT EXISTS event_log (
    timestamp REAL NOT NULL,
    type TEXT NOT NULL,
    event_tx_id INTEGER NOT NULL,
    old_ref TEXT,
    new_ref TEXT,
    ref_name TEXT,
    message TEXT
)
",
        rusqlite::params![],
    )
    .wrap_err("Creating `event_log` table")?;

    conn.execute(
        "
CREATE TABLE IF NOT EXISTS event_transactions (
    timestamp REAL NOT NULL,

    -- Set as `PRIMARY KEY` to have SQLite select a value automatically. Set as
    -- `AUTOINCREMENT` to ensure that SQLite doesn't reuse the value later if a
    -- row is deleted. (We don't plan to delete rows right now, but maybe
    -- later?)
    event_tx_id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,

    message TEXT
)
",
        rusqlite::params![],
    )
    .wrap_err("Creating `event_transactions` table")?;

    Ok(())
}

impl<'conn> EventLogDb<'conn> {
    /// Constructor.
    #[instrument]
    pub fn new(conn: &'conn rusqlite::Connection) -> eyre::Result<Self> {
        init_tables(conn)?;
        Ok(EventLogDb { conn })
    }

    /// Add events in the given order to the database, in a transaction.
    ///
    /// Args:
    /// * events: The events to add.
    #[instrument]
    pub fn add_events(&self, events: Vec<Event>) -> eyre::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for event in events {
            let row = match Row::try_from(event) {
                Ok(row) => row,
                Err(()) => continue,
            };
            let Row {
                timestamp,
                type_,
                event_tx_id,
                ref1,
                ref2,
                ref_name,
                message,
            } = row;

            let ref1 = ref1.as_ref().map(|x| x.as_str());
            let ref2 = ref2.as_ref().map(|x| x.as_str());
            let ref_name = ref_name.as_ref().map(|x| x.as_str());
            let message = message.as_ref().map(|x| x.as_str());

            tx.execute(
                "
INSERT INTO event_log VALUES (
    :timestamp,
    :type,
    :event_tx_id,
    :old_ref,
    :new_ref,
    :ref_name,
    :message
)
            ",
                rusqlite::named_params! {
                    ":timestamp": timestamp,
                    ":type": &type_,
                    ":event_tx_id": event_tx_id,
                    ":old_ref": &ref1,
                    ":new_ref": &ref2,
                    ":ref_name": &ref_name,
                    ":message": &message,
                },
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Get all the events in the database.
    ///
    /// Returns: All the events in the database, ordered from oldest to newest.
    #[instrument]
    pub fn get_events(&self) -> eyre::Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "
SELECT timestamp, type, event_tx_id, old_ref, new_ref, ref_name, message
FROM event_log
ORDER BY rowid ASC
",
        )?;
        let rows: rusqlite::Result<Vec<Row>> = stmt
            .query_map(rusqlite::params![], |row| {
                let timestamp: f64 = row.get("timestamp")?;
                let event_tx_id: isize = row.get("event_tx_id")?;
                let type_: String = row.get("type")?;
                let ref_name: Option<String> = row.get("ref_name")?;
                let old_ref: Option<String> = row.get("old_ref")?;
                let new_ref: Option<String> = row.get("new_ref")?;
                let message: Option<String> = row.get("message")?;

                Ok(Row {
                    timestamp,
                    event_tx_id,
                    type_,
                    ref_name: ref_name.map(ReferenceName::from),
                    ref1: old_ref.map(ReferenceName::from),
                    ref2: new_ref.map(ReferenceName::from),
                    message: message.map(ReferenceName::from),
                })
            })?
            .collect();
        let rows = rows?;
        rows.into_iter().map(Event::try_from).collect()
    }

    #[instrument]
    fn make_transaction_id_inner(
        &self,
        now: SystemTime,
        message: &str,
    ) -> eyre::Result<EventTransactionId> {
        if let Ok(transaction_id) = std::env::var(BRANCHLESS_TRANSACTION_ID_ENV_VAR) {
            if let Ok(transaction_id) = transaction_id.parse::<EventTransactionId>() {
                return Ok(transaction_id);
            }
        }

        let tx = self.conn.unchecked_transaction()?;

        let timestamp = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .wrap_err("Calculating event transaction timestamp")?
            .as_secs_f64();
        self.conn
            .execute(
                "
            INSERT INTO event_transactions
            (timestamp, message)
            VALUES
            (:timestamp, :message)
        ",
                rusqlite::named_params! {
                    ":timestamp": timestamp,
                    ":message": message,
                },
            )
            .wrap_err("Creating event transaction")?;

        // Ensure that we query `last_insert_rowid` in a transaction, in case
        // there's another thread in this process making queries with the same
        // SQLite connection.
        let event_tx_id: isize = self.conn.last_insert_rowid().try_into()?;
        tx.commit()?;
        Ok(EventTransactionId::Id(event_tx_id))
    }

    /// Create a new event transaction ID to be used to insert subsequent
    /// `Event`s into the database.
    pub fn make_transaction_id(
        &self,
        now: SystemTime,
        message: impl AsRef<str>,
    ) -> eyre::Result<EventTransactionId> {
        self.make_transaction_id_inner(now, message.as_ref())
    }

    /// Get the message associated with the given transaction.
    pub fn get_transaction_message(&self, event_tx_id: EventTransactionId) -> eyre::Result<String> {
        let event_tx_id = match event_tx_id {
            EventTransactionId::Id(event_tx_id) => event_tx_id,
            EventTransactionId::Suppressed => {
                eyre::bail!("No message available for suppressed transaction ID")
            }
        };
        let mut stmt = self.conn.prepare(
            "
SELECT message
FROM event_transactions
WHERE event_tx_id = :event_tx_id
",
        )?;
        let result: String = stmt.query_row(
            rusqlite::named_params![":event_tx_id": event_tx_id,],
            |row| {
                let message: String = row.get("message")?;
                Ok(message)
            },
        )?;
        Ok(result)
    }
}

/// Determine whether a given reference is used to keep a commit alive.
///
/// Returns: Whether or not the given reference is used internally to keep the
/// commit alive, so that it's not collected by Git's garbage collection
/// mechanism.
pub fn is_gc_ref(reference_name: &ReferenceName) -> bool {
    reference_name.as_str().starts_with("refs/branchless/")
}

/// Determines whether or not updates to the given reference should be ignored.
///
/// Returns: Whether or not updates to the given reference should be ignored.
pub fn should_ignore_ref_updates(reference_name: &ReferenceName) -> bool {
    if is_gc_ref(reference_name) {
        return true;
    }

    matches!(
        reference_name.as_str(),
        "ORIG_HEAD"
            | "CHERRY_PICK"
            | "REBASE_HEAD"
            | "CHERRY_PICK_HEAD"
            // From Git's `is_special_ref` in `refs.c`:
            | "AUTO_MERGE"
            | "FETCH_HEAD"
    )
}

#[derive(Debug)]
enum EventClassification {
    Show,
    Hide,
}

/// Whether or not a commit is considered active.
///
/// This is determined by the last `Event` that affected the commit. If no
/// activity has been observed for a commit, it's considered inactive.
#[derive(Debug)]
pub enum CommitActivityStatus {
    /// The commit is active, and should be rendered as part of the commit graph.
    Active,

    /// No history has been observed for this commit, so it's inactive, and
    /// should not be rendered as part of the commit graph (unless a descendant
    /// commit is visible).
    Inactive,

    /// The commit has been obsoleted by a user event (rewriting or an explicit
    /// request to obsolete the commit), and should be hidden from the commit
    /// graph (unless a descendant commit is visible).
    Obsolete,
}

#[derive(Debug)]
struct EventInfo {
    id: isize,
    event: Event,
    event_classification: EventClassification,
}

/// Events up to this cursor (exclusive) are available to the caller.
///
/// The "event cursor" is used to move the event replayer forward or
/// backward in time, so as to show the state of the repository at that
/// time.
///
/// The cursor is a position in between two events in the event log.
/// Thus, all events before to the cursor are considered to be in effect,
/// and all events after the cursor are considered to not have happened
/// yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventCursor {
    event_id: isize,
}

/// Processes events in order and determine the repo's visible commits.
pub struct EventReplayer {
    /// Events are numbered starting from zero.
    id_counter: isize,

    /// The list of observed events.
    events: Vec<Event>,

    /// The name of the reference representing the main branch.
    main_branch_reference_name: ReferenceName,

    /// The events that have affected each commit.
    commit_history: HashMap<NonZeroOid, Vec<EventInfo>>,

    /// Map from ref names to ref locations (an OID or another ref name). Works
    /// around <https://github.com/arxanas/git-branchless/issues/7>.
    ///
    /// If an entry is not present, it was either never observed, or it most
    /// recently changed to point to the zero hash (i.e. it was deleted).
    ref_locations: HashMap<ReferenceName, NonZeroOid>,
}

impl std::fmt::Debug for EventReplayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "<EventReplayer events.len={:?} ref_locations.len={:?}>",
            self.events.len(),
            self.ref_locations.len()
        )
    }
}

impl EventReplayer {
    fn new(main_branch_reference_name: ReferenceName) -> Self {
        EventReplayer {
            id_counter: 0,
            events: vec![],
            main_branch_reference_name,
            commit_history: HashMap::new(),
            ref_locations: HashMap::new(),
        }
    }

    /// Construct the replayer from all the events in the database.
    ///
    /// Args:
    /// * `event_log_db`: The database to query events from.
    ///
    /// Returns: The constructed replayer.
    #[instrument]
    pub fn from_event_log_db(
        effects: &Effects,
        repo: &Repo,
        event_log_db: &EventLogDb,
    ) -> eyre::Result<Self> {
        let (_effects, _progress) = effects.start_operation(OperationType::ProcessEvents);

        let main_branch_reference_name = repo.get_main_branch()?.get_reference_name()?;
        let mut result = EventReplayer::new(main_branch_reference_name);
        for event in event_log_db.get_events()? {
            result.process_event(&event);
        }
        Ok(result)
    }

    /// Process the given event.
    ///
    /// This also sets the event cursor to point to immediately after the event
    /// that was just processed.
    ///
    /// Args:
    /// * `event`: The next event to process. Events should be passed to the
    /// * `replayer` in order from oldest to newest.
    pub fn process_event(&mut self, event: &Event) {
        // Drop non-meaningful ref-update events.
        if let Event::RefUpdateEvent { ref_name, .. } = event {
            if should_ignore_ref_updates(ref_name) {
                return;
            }
        }

        let event = match self.fix_event_git_v2_31(event.clone()) {
            None => {
                return;
            }
            Some(event) => {
                self.events.push(event);
                self.events.last().unwrap()
            }
        };
        let id = self.id_counter;
        self.id_counter += 1;

        match &event {
            Event::RewriteEvent {
                timestamp: _,
                event_tx_id: _,
                old_commit_oid,
                new_commit_oid,
            } => {
                if let MaybeZeroOid::NonZero(old_commit_oid) = old_commit_oid {
                    self.commit_history
                        .entry(*old_commit_oid)
                        .or_default()
                        .push(EventInfo {
                            id,
                            event: event.clone(),
                            event_classification: EventClassification::Hide,
                        });
                }
                if let MaybeZeroOid::NonZero(new_commit_oid) = new_commit_oid {
                    self.commit_history
                        .entry(*new_commit_oid)
                        .or_default()
                        .push(EventInfo {
                            id,
                            event: event.clone(),
                            event_classification: EventClassification::Show,
                        });
                }
            }

            // A reference update doesn't indicate a change to a commit, so we
            // don't include it in the `commit_history`. We'll traverse the
            // history later to find historical locations of references when
            // needed.
            Event::RefUpdateEvent {
                ref_name, new_oid, ..
            } => match new_oid {
                MaybeZeroOid::NonZero(new_oid) => {
                    self.ref_locations.insert(ref_name.clone(), *new_oid);
                }
                MaybeZeroOid::Zero => {
                    self.ref_locations.remove(ref_name);
                }
            },

            Event::CommitEvent {
                timestamp: _,
                event_tx_id: _,
                commit_oid,
            } => self
                .commit_history
                .entry(*commit_oid)
                .or_default()
                .push(EventInfo {
                    id,
                    event: event.clone(),
                    event_classification: EventClassification::Show,
                }),

            Event::ObsoleteEvent {
                timestamp: _,
                event_tx_id: _,
                commit_oid,
            } => self
                .commit_history
                .entry(*commit_oid)
                .or_default()
                .push(EventInfo {
                    id,
                    event: event.clone(),
                    event_classification: EventClassification::Hide,
                }),

            Event::UnobsoleteEvent {
                timestamp: _,
                event_tx_id: _,
                commit_oid,
            } => self
                .commit_history
                .entry(*commit_oid)
                .or_default()
                .push(EventInfo {
                    id,
                    event: event.clone(),
                    event_classification: EventClassification::Show,
                }),

            Event::WorkingCopySnapshot { .. } => {
                // Do nothing. A working copy snapshot doesn't imply that the
                // commit has become active or inactive.
            }
        };
    }

    /// See https://github.com/arxanas/git-branchless/issues/7.
    fn fix_event_git_v2_31(&self, event: Event) -> Option<Event> {
        let event = match event {
            // Git v2.31 will sometimes fail to set the `old_ref` field when
            // deleting refs. This means that undoing the operation later
            // becomes incorrect, as we just swap the `old_ref` and `new_ref`
            // values.
            Event::RefUpdateEvent {
                timestamp,
                event_tx_id,
                ref_name,
                old_oid: MaybeZeroOid::Zero,
                new_oid: MaybeZeroOid::Zero,
                message,
            } => {
                let old_oid: MaybeZeroOid = self.ref_locations.get(&ref_name).copied().into();
                Event::RefUpdateEvent {
                    timestamp,
                    event_tx_id,
                    ref_name,
                    old_oid,
                    new_oid: MaybeZeroOid::Zero,
                    message,
                }
            }

            _ => event,
        };

        match (event, self.events.last()) {
            // Sometimes, Git v2.31 will issue multiple delete reference
            // transactions (one for the unpacked refs, and one for the packed
            // refs). Ignore the duplicate second one, for determinism in
            // testing. See https://lore.kernel.org/git/YFMCLSdImkW3B1rM@ncase/
            // for more details.
            (
                Event::RefUpdateEvent {
                    timestamp: _,
                    event_tx_id: _,
                    ref ref_name,
                    old_oid: _,
                    new_oid: MaybeZeroOid::Zero,
                    ref message,
                },
                Some(Event::RefUpdateEvent {
                    timestamp: _,
                    event_tx_id: _,
                    ref_name: last_ref_name,
                    old_oid: _,
                    new_oid: MaybeZeroOid::Zero,
                    message: last_message,
                }),
            ) if ref_name == last_ref_name && message == last_message => None,

            (event, _) => Some(event),
        }
    }

    fn get_cursor_commit_history(&self, cursor: EventCursor, oid: NonZeroOid) -> Vec<&EventInfo> {
        match self.commit_history.get(&oid) {
            None => vec![],
            Some(history) => history
                .iter()
                .filter(|event_info| event_info.id < cursor.event_id)
                .collect(),
        }
    }

    /// Determines whether a commit is considered "active" at the cursor's point
    /// in time.
    pub fn get_cursor_commit_activity_status(
        &self,
        cursor: EventCursor,
        oid: NonZeroOid,
    ) -> CommitActivityStatus {
        let history = self.get_cursor_commit_history(cursor, oid);
        match history.last() {
            Some(EventInfo {
                id: _,
                event: _,
                event_classification: EventClassification::Show,
            }) => CommitActivityStatus::Active,

            Some(EventInfo {
                id: _,
                event: _,
                event_classification: EventClassification::Hide,
            }) => CommitActivityStatus::Obsolete,

            None => CommitActivityStatus::Inactive,
        }
    }

    /// Get the latest event affecting a given commit, as of the cursor's point
    /// in time.
    ///
    /// Args:
    /// * `oid`: The OID of the commit to check.
    ///
    /// Returns: The most recent event that affected that commit. If this commit
    /// was not observed by the replayer, returns `None`.
    pub fn get_cursor_commit_latest_event(
        &self,
        cursor: EventCursor,
        oid: NonZeroOid,
    ) -> Option<&Event> {
        let history = self.get_cursor_commit_history(cursor, oid);
        let event_info = *history.last()?;
        Some(&event_info.event)
    }

    /// Get all OIDs which have been observed so far. This should be the set of
    /// non-inactive commits.
    pub fn get_cursor_oids(&self, cursor: EventCursor) -> HashSet<NonZeroOid> {
        self.commit_history
            .iter()
            .filter_map(|(oid, history)| {
                if history.iter().any(|event| event.id < cursor.event_id) {
                    Some(*oid)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Create an event cursor pointing to immediately after the last event.
    pub fn make_default_cursor(&self) -> EventCursor {
        self.make_cursor(self.events.len().try_into().unwrap())
    }

    /// Create an event cursor pointing to immediately after the provided event ID.
    ///
    /// If the event ID is too low or too high, it will be clamped to the valid
    /// range for event IDs.
    pub fn make_cursor(&self, event_id: isize) -> EventCursor {
        let event_id = if event_id < 0 { 0 } else { event_id };
        let num_events: isize = self.events.len().try_into().unwrap();
        let event_id = if event_id > num_events {
            num_events
        } else {
            event_id
        };
        EventCursor { event_id }
    }

    /// Advance the event cursor by the specified number of events.
    ///
    /// Args:
    /// * `num_events`: The number of events to advance by. Can be positive,
    /// zero, or negative. If out of bounds, the cursor is set to the first or
    /// last valid position, as appropriate.
    pub fn advance_cursor(&self, cursor: EventCursor, num_events: isize) -> EventCursor {
        self.make_cursor(cursor.event_id + num_events)
    }

    fn get_event_tx_id_before_cursor(&self, cursor: EventCursor) -> Option<EventTransactionId> {
        self.get_event_before_cursor(cursor)
            .map(|(_event_id, event)| event.get_event_tx_id())
    }

    /// The event cursor may not be between two events with different transaction
    /// IDs (that is, it may not be perfectly in between transactions). Move the
    /// cursor forward until it is at the boundary of two transactions
    fn snap_to_transaction_boundary(&self, cursor: EventCursor) -> EventCursor {
        let next_cursor = self.advance_cursor(cursor, 1);
        if cursor == next_cursor {
            return cursor;
        }
        let current_tx_id = self.get_event_tx_id_before_cursor(cursor);
        let next_tx_id = self.get_event_tx_id_before_cursor(next_cursor);
        if current_tx_id == next_tx_id {
            self.snap_to_transaction_boundary(next_cursor)
        } else {
            cursor
        }
    }

    fn advance_cursor_by_transaction_helper(
        &self,
        cursor: EventCursor,
        num_transactions: isize,
    ) -> EventCursor {
        match num_transactions.cmp(&0) {
            Ordering::Equal => self.snap_to_transaction_boundary(cursor),
            Ordering::Greater => {
                let next_cursor = self.advance_cursor(cursor, 1);
                if cursor == next_cursor {
                    return next_cursor;
                }
                let current_tx_id = self.get_event_tx_id_before_cursor(cursor);
                let next_tx_id = self.get_event_tx_id_before_cursor(next_cursor);
                let num_transactions = if current_tx_id == next_tx_id {
                    num_transactions
                } else {
                    num_transactions - 1
                };
                self.advance_cursor_by_transaction_helper(next_cursor, num_transactions)
            }
            Ordering::Less => {
                let prev_cursor = self.advance_cursor(cursor, -1);
                if cursor == prev_cursor {
                    return prev_cursor;
                }
                let current_tx_id = self.get_event_tx_id_before_cursor(cursor);
                let prev_tx_id = self.get_event_tx_id_before_cursor(prev_cursor);
                let num_transactions = if current_tx_id == prev_tx_id {
                    num_transactions
                } else {
                    num_transactions + 1
                };
                self.advance_cursor_by_transaction_helper(prev_cursor, num_transactions)
            }
        }
    }

    /// Advance the cursor to the transaction which is `num_transactions` after
    /// the current cursor. `num_transactions` can be negative.
    ///
    /// The returned cursor will point to the position immediately after the last
    /// event in the subsequent transaction.
    pub fn advance_cursor_by_transaction(
        &self,
        cursor: EventCursor,
        num_transactions: isize,
    ) -> EventCursor {
        if self.events.is_empty() {
            cursor
        } else {
            let cursor = self.snap_to_transaction_boundary(cursor);
            self.advance_cursor_by_transaction_helper(cursor, num_transactions)
        }
    }

    /// Get the OID of `HEAD` at the cursor's point in time.
    ///
    /// Returns: The OID pointed to by `HEAD` at that time, or `None` if `HEAD`
    /// was never observed.
    fn get_cursor_head_oid(&self, cursor: EventCursor) -> Option<NonZeroOid> {
        let cursor_event_id: usize = cursor.event_id.try_into().unwrap();
        self.events[0..cursor_event_id]
            .iter()
            .rev()
            .find_map(|event| {
                match &event {
                    Event::RefUpdateEvent {
                        ref_name,
                        new_oid: MaybeZeroOid::NonZero(new_oid),
                        ..
                    } if ref_name.as_str() == "HEAD" => Some(*new_oid),
                    Event::RefUpdateEvent { .. } => None,

                    // Not strictly necessary, but helps to compensate in case
                    // the user is not running Git v2.29 or above, and therefore
                    // doesn't have the corresponding `RefUpdateEvent`.
                    Event::CommitEvent { commit_oid, .. } => Some(*commit_oid),

                    Event::WorkingCopySnapshot {
                        head_oid: MaybeZeroOid::NonZero(head_oid),
                        ..
                    } => Some(*head_oid),
                    Event::WorkingCopySnapshot {
                        head_oid: MaybeZeroOid::Zero,
                        ..
                    } => None,

                    Event::RewriteEvent { .. }
                    | Event::ObsoleteEvent { .. }
                    | Event::UnobsoleteEvent { .. } => None,
                }
            })
    }

    fn get_cursor_branch_oid(
        &self,
        cursor: EventCursor,
        reference_name: &ReferenceName,
    ) -> eyre::Result<Option<NonZeroOid>> {
        let cursor_event_id: usize = cursor.event_id.try_into().unwrap();
        let oid = self.events[0..cursor_event_id]
            .iter()
            .rev()
            .find_map(|event| match &event {
                Event::RefUpdateEvent {
                    ref_name,
                    new_oid: MaybeZeroOid::NonZero(new_oid),
                    ..
                } if ref_name == reference_name => Some(*new_oid),
                _ => None,
            });
        Ok(oid)
    }

    /// Get the OID of the main branch at the cursor's point in time.
    ///
    /// Note that this doesn't handle the case of the user having changed their
    /// main branch configuration. That is, if it was previously `master`, and
    /// then changed to `main`, we will show only the historical locations of
    /// `main`, and never `master`.
    ///
    /// Args:
    /// * `repo`: The Git repository.
    ///
    /// Returns: A mapping from an OID to the names of branches pointing to that
    /// OID.
    #[instrument]
    fn get_cursor_main_branch_oid(
        &self,
        cursor: EventCursor,
        repo: &Repo,
    ) -> eyre::Result<NonZeroOid> {
        let main_branch_reference_name = repo.get_main_branch()?.get_reference_name()?;
        let main_branch_oid = self.get_cursor_branch_oid(cursor, &main_branch_reference_name)?;
        match main_branch_oid {
            Some(main_branch_oid) => Ok(main_branch_oid),
            None => {
                // Assume the main branch just hasn't been observed moving yet,
                // so its value at the current time is fine to use.
                repo.get_main_branch_oid()
            }
        }
    }

    /// Get the mapping of branch OIDs to names at the cursor's point in
    /// time.
    ///
    /// Same as `get_branch_oid_to_names`, but for a previous point in time.
    ///
    /// Args:
    /// * `repo`: The Git repository.
    ///
    /// Returns: A mapping from an OID to the names of branches pointing to that
    /// OID.
    fn get_cursor_branch_oid_to_names(
        &self,
        cursor: EventCursor,
        repo: &Repo,
    ) -> eyre::Result<HashMap<NonZeroOid, HashSet<ReferenceName>>> {
        let mut ref_name_to_oid: HashMap<&ReferenceName, NonZeroOid> = HashMap::new();
        let cursor_event_id: usize = cursor.event_id.try_into().unwrap();
        for event in self.events[..cursor_event_id].iter() {
            match event {
                Event::RefUpdateEvent {
                    new_oid: MaybeZeroOid::NonZero(new_oid),
                    ref_name,
                    ..
                } => {
                    ref_name_to_oid.insert(ref_name, *new_oid);
                }
                Event::RefUpdateEvent {
                    new_oid: MaybeZeroOid::Zero,
                    ref_name,
                    ..
                } => {
                    ref_name_to_oid.remove(ref_name);
                }
                _ => {}
            }
        }

        let mut result: HashMap<NonZeroOid, HashSet<ReferenceName>> = HashMap::new();
        for (ref_name, ref_oid) in ref_name_to_oid.iter() {
            if let CategorizedReferenceName::LocalBranch { .. } =
                CategorizedReferenceName::new(ref_name)
            {
                result
                    .entry(*ref_oid)
                    .or_default()
                    .insert((*ref_name).clone());
            }
        }

        let main_branch_oid = self.get_cursor_main_branch_oid(cursor, repo)?;
        result
            .entry(main_branch_oid)
            .or_default()
            .insert(self.main_branch_reference_name.clone());
        Ok(result)
    }

    /// Get the `RepoReferencesSnapshot` at the cursor's point in time.
    pub fn get_references_snapshot(
        &self,
        repo: &Repo,
        cursor: EventCursor,
    ) -> eyre::Result<RepoReferencesSnapshot> {
        let head_oid = self.get_cursor_head_oid(cursor);
        let main_branch_oid = self.get_cursor_main_branch_oid(cursor, repo)?;
        let branch_oid_to_names = self.get_cursor_branch_oid_to_names(cursor, repo)?;
        Ok(RepoReferencesSnapshot {
            head_oid,
            main_branch_oid,
            branch_oid_to_names,
        })
    }

    /// Get the event immediately before the cursor.
    ///
    /// Returns: A tuple of event ID and the event that most recently happened.
    /// If no event was before the event cursor, returns `None` instead.
    pub fn get_event_before_cursor(&self, cursor: EventCursor) -> Option<(isize, &Event)> {
        if cursor.event_id == 0 {
            None
        } else {
            let previous_cursor_event_id: usize = (cursor.event_id - 1).try_into().unwrap();
            Some((cursor.event_id, &self.events[previous_cursor_event_id]))
        }
    }

    /// Get all the events in the transaction immediately before the cursor.
    ///
    /// Returns: A tuple of event ID and the events that happened in the most
    /// recent transaction. The event ID corresponds to the ID of the first event
    /// in the returned list of events. If there were no events before the event
    /// cursor (and therefore no transactions), returns `None` instead.
    pub fn get_tx_events_before_cursor(&self, cursor: EventCursor) -> Option<(isize, &[Event])> {
        let prev_tx_cursor = self.advance_cursor_by_transaction(cursor, -1);
        let EventCursor {
            event_id: prev_event_id,
        } = prev_tx_cursor;
        let EventCursor {
            event_id: curr_event_id,
        } = cursor;
        let tx_events =
            &self.events[prev_event_id.try_into().unwrap()..curr_event_id.try_into().unwrap()];
        match tx_events {
            [] => None,
            events => Some((prev_event_id + 1, events)),
        }
    }

    /// Get all the events that have happened since the event cursor.
    ///
    /// Returns: An ordered list of events that have happened since the event
    /// cursor, from least recent to most recent.
    pub fn get_events_since_cursor(&self, cursor: EventCursor) -> &[Event] {
        let cursor_event_id: usize = cursor.event_id.try_into().unwrap();
        &self.events[cursor_event_id..]
    }
}

/// Testing helpers.
pub mod testing {
    use super::*;

    /// Create a new `EventReplayer`, for testing.
    pub fn new_event_replayer(main_branch_reference_name: ReferenceName) -> EventReplayer {
        EventReplayer::new(main_branch_reference_name)
    }

    /// Create a new transaction ID, for testing.
    pub fn new_event_transaction_id(id: isize) -> EventTransactionId {
        EventTransactionId::Id(id)
    }

    /// Create a new event cursor, for testing.
    pub fn new_event_cursor(event_id: isize) -> EventCursor {
        EventCursor { event_id }
    }

    /// Remove the timestamp for the event, for determinism in testing.
    pub fn redact_event_timestamp(mut event: Event) -> Event {
        match event {
            Event::RewriteEvent {
                ref mut timestamp, ..
            }
            | Event::RefUpdateEvent {
                ref mut timestamp, ..
            }
            | Event::CommitEvent {
                ref mut timestamp, ..
            }
            | Event::ObsoleteEvent {
                ref mut timestamp, ..
            }
            | Event::UnobsoleteEvent {
                ref mut timestamp, ..
            }
            | Event::WorkingCopySnapshot {
                ref mut timestamp, ..
            } => *timestamp = 0.0,
        }
        event
    }

    /// Get the events stored inside an `EventReplayer`.
    pub fn get_event_replayer_events(event_replayer: &EventReplayer) -> &Vec<Event> {
        &event_replayer.events
    }
}
