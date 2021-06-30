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

use anyhow::Context;
use fn_error_context::context;
use log::warn;

use crate::core::config::get_main_branch_name;
use crate::util::wrap_git_error;

use super::repo::Repo;

/// When this environment variable is set, we reuse the ID for the transaction
/// which the caller has already started.
pub const BRANCHLESS_TRANSACTION_ID_ENV_VAR: &str = "BRANCHLESS_TRANSACTION_ID";

// Wrapper around the row stored directly in the database.
struct Row {
    timestamp: f64,
    type_: String,
    event_tx_id: isize,
    ref1: Option<String>,
    ref2: Option<String>,
    ref_name: Option<String>,
    message: Option<String>,
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventTransactionId(isize);

impl ToString for EventTransactionId {
    fn to_string(&self) -> String {
        let EventTransactionId(event_id) = self;
        event_id.to_string()
    }
}

impl FromStr for EventTransactionId {
    type Err = <isize as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let event_id = s.parse()?;
        Ok(EventTransactionId(event_id))
    }
}

/// An event that occurred to one of the commits in the repository.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Indicates that the commit was rewritten.
    ///
    /// Examples of rewriting include rebases and amended commits.
    ///
    /// We typically want to mark the new version of the commit as visible and
    /// the old version of the commit as hidden.
    RewriteEvent {
        /// The timestamp of the event.
        timestamp: f64,

        /// The transaction ID of the event.
        event_tx_id: EventTransactionId,

        /// The OID of the commit before the rewrite.
        old_commit_oid: git2::Oid,

        /// The OID of the commit after the rewrite.
        new_commit_oid: git2::Oid,
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
        ref_name: String,

        /// The old referent.
        ///
        /// May be an OID (in the case of a direct reference) or another
        /// reference name (in the case of a symbolic reference).
        old_ref: Option<String>,

        /// The updated referent.
        ///
        /// This may not be different from the old referent.
        ///
        /// May be an OID (in the case of a direct reference) or another
        /// reference name (in the case of a symbolic reference).
        new_ref: Option<String>,

        /// A message associated with the rewrite, if any.
        message: Option<String>,
    },

    /// Indicate that the user made a commit.
    ///
    /// User commits should be marked as visible.
    CommitEvent {
        /// The timestamp of the event.
        timestamp: f64,

        /// The transaction ID of the event.
        event_tx_id: EventTransactionId,

        /// The new commit OID.
        commit_oid: git2::Oid,
    },

    /// Indicates that a commit was explicitly hidden by the user.
    ///
    /// If the commit in question was not already visible, then this has no
    /// practical effect.
    HideEvent {
        /// The timestamp of the event.
        timestamp: f64,

        /// The transaction ID of the event.
        event_tx_id: EventTransactionId,

        /// The OID of the commit that was hidden.
        commit_oid: git2::Oid,
    },

    /// Indicates that a commit was explicitly un-hidden by the user.
    ///
    /// If the commit in question was not already hidden, then this has no
    /// practical effect.
    UnhideEvent {
        /// The timestamp of the event.
        timestamp: f64,

        /// The transaction ID of the event.
        event_tx_id: EventTransactionId,

        /// The OID of the commit that was unhidden.
        commit_oid: git2::Oid,
    },
}

impl Event {
    /// Get the timestamp associated with this event.
    pub fn get_timestamp(&self) -> SystemTime {
        let timestamp = match self {
            Event::RewriteEvent { timestamp, .. } => timestamp,
            Event::RefUpdateEvent { timestamp, .. } => timestamp,
            Event::CommitEvent { timestamp, .. } => timestamp,
            Event::HideEvent { timestamp, .. } => timestamp,
            Event::UnhideEvent { timestamp, .. } => timestamp,
        };
        SystemTime::UNIX_EPOCH + Duration::from_secs_f64(*timestamp)
    }

    /// Get the event transaction ID associated with this event.
    pub fn get_event_tx_id(&self) -> EventTransactionId {
        match self {
            Event::RewriteEvent { event_tx_id, .. } => *event_tx_id,
            Event::RefUpdateEvent { event_tx_id, .. } => *event_tx_id,
            Event::CommitEvent { event_tx_id, .. } => *event_tx_id,
            Event::HideEvent { event_tx_id, .. } => *event_tx_id,
            Event::UnhideEvent { event_tx_id, .. } => *event_tx_id,
        }
    }
}

impl From<Event> for Row {
    fn from(event: Event) -> Row {
        match event {
            Event::RewriteEvent {
                timestamp,
                event_tx_id: EventTransactionId(event_tx_id),
                old_commit_oid,
                new_commit_oid,
            } => Row {
                timestamp,
                event_tx_id,
                type_: String::from("rewrite"),
                ref1: Some(old_commit_oid.to_string()),
                ref2: Some(new_commit_oid.to_string()),
                ref_name: None,
                message: None,
            },

            Event::RefUpdateEvent {
                timestamp,
                event_tx_id: EventTransactionId(event_tx_id),
                ref_name,
                old_ref,
                new_ref,
                message,
            } => Row {
                timestamp,
                event_tx_id,
                type_: String::from("ref-move"),
                ref1: old_ref,
                ref2: new_ref,
                ref_name: Some(ref_name),
                message,
            },

            Event::CommitEvent {
                timestamp,
                event_tx_id: EventTransactionId(event_tx_id),
                commit_oid,
            } => Row {
                timestamp,
                event_tx_id,
                type_: String::from("commit"),
                ref1: Some(commit_oid.to_string()),
                ref2: None,
                ref_name: None,
                message: None,
            },

            Event::HideEvent {
                timestamp,
                event_tx_id: EventTransactionId(event_tx_id),
                commit_oid,
            } => Row {
                timestamp,
                event_tx_id,
                type_: String::from("hide"),
                ref1: Some(commit_oid.to_string()),
                ref2: None,
                ref_name: None,
                message: None,
            },

            Event::UnhideEvent {
                timestamp,
                event_tx_id: EventTransactionId(event_tx_id),
                commit_oid,
            } => Row {
                timestamp,
                event_tx_id,
                type_: String::from("unhide"),
                ref1: Some(commit_oid.to_string()),
                ref2: None,
                ref_name: None,
                message: None,
            },
        }
    }
}

impl TryFrom<Row> for Event {
    type Error = anyhow::Error;

    #[context("Converting database result row into `Event`")]
    fn try_from(row: Row) -> Result<Self, Self::Error> {
        let Row {
            timestamp,
            event_tx_id,
            type_,
            ref_name,
            ref1,
            ref2,
            message,
        } = row;
        let event_tx_id = EventTransactionId(event_tx_id);

        let get_oid = |oid: &Option<String>, oid_name: &str| -> anyhow::Result<git2::Oid> {
            match oid {
                Some(oid) => {
                    let oid = git2::Oid::from_str(&oid).map_err(wrap_git_error)?;
                    Ok(oid)
                }
                None => Err(anyhow::anyhow!(
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
                    ref_name.ok_or_else(|| anyhow::anyhow!("ref-move event missing ref name"))?;
                Event::RefUpdateEvent {
                    timestamp,
                    event_tx_id,
                    ref_name,
                    old_ref: ref1,
                    new_ref: ref2,
                    message,
                }
            }

            "commit" => {
                let commit_oid = get_oid(&ref1, "commit OID")?;
                Event::CommitEvent {
                    timestamp,
                    event_tx_id,
                    commit_oid,
                }
            }

            "hide" => {
                let commit_oid = get_oid(&ref1, "commit OID")?;
                Event::HideEvent {
                    timestamp,
                    event_tx_id,
                    commit_oid,
                }
            }

            "unhide" => {
                let commit_oid = get_oid(&ref1, "commit OID")?;
                Event::UnhideEvent {
                    timestamp,
                    event_tx_id,
                    commit_oid,
                }
            }

            other => anyhow::bail!("Unknown event type {}", other),
        };
        Ok(event)
    }
}

/// Stores `Event`s on disk.
pub struct EventLogDb<'conn> {
    conn: &'conn rusqlite::Connection,
}

#[context("Initializing `EventLogDb` tables")]
fn init_tables(conn: &rusqlite::Connection) -> anyhow::Result<()> {
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
    .context("Creating `event_log` table")?;

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
    .context("Creating `event_transactions` table")?;

    Ok(())
}

impl<'conn> EventLogDb<'conn> {
    /// Constructor.
    #[context("Constructing `EventLogDb`")]
    pub fn new(conn: &'conn rusqlite::Connection) -> anyhow::Result<Self> {
        init_tables(&conn)?;
        Ok(EventLogDb { conn })
    }

    /// Add events in the given order to the database, in a transaction.
    ///
    /// Args:
    /// * events: The events to add.
    #[context("Adding events to event-log")]
    pub fn add_events(&mut self, events: Vec<Event>) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for event in events {
            let Row {
                timestamp,
                type_,
                event_tx_id,
                ref1,
                ref2,
                ref_name,
                message,
            } = Row::from(event);
            tx.execute_named(
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
    #[context("Querying events from `EventLogDb`")]
    pub fn get_events(&self) -> anyhow::Result<Vec<Event>> {
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

                // A ref name corresponding to commit hash `0` indicates that
                // there was no old/new ref at all (i.e. it was created or deleted).
                let old_ref = old_ref.filter(|old_ref| *old_ref != git2::Oid::zero().to_string());
                let new_ref = new_ref.filter(|new_ref| *new_ref != git2::Oid::zero().to_string());

                Ok(Row {
                    timestamp,
                    event_tx_id,
                    type_,
                    ref_name,
                    ref1: old_ref,
                    ref2: new_ref,
                    message,
                })
            })?
            .collect();
        let rows = rows?;
        rows.into_iter().map(Event::try_from).collect()
    }

    /// Create a new event transaction ID to be used to insert subsequent
    /// `Event`s into the database.
    #[context("Creating a new `EventTransactionId`")]
    pub fn make_transaction_id(
        &self,
        now: SystemTime,
        message: impl AsRef<str>,
    ) -> anyhow::Result<EventTransactionId> {
        if let Ok(transaction_id) = std::env::var(BRANCHLESS_TRANSACTION_ID_ENV_VAR) {
            if let Ok(transaction_id) = transaction_id.parse::<EventTransactionId>() {
                return Ok(transaction_id);
            }
        }

        let tx = self.conn.unchecked_transaction()?;

        let timestamp = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .with_context(|| format!("Calculating event transaction timestamp: {:?}", &now))?
            .as_secs_f64();
        self.conn
            .execute_named(
                "
            INSERT INTO event_transactions
            (timestamp, message)
            VALUES
            (:timestamp, :message)
        ",
                rusqlite::named_params! {
                    ":timestamp": timestamp,
                    ":message": message.as_ref(),
                },
            )
            .with_context(|| {
                format!(
                    "Creating event transaction (now: {:?}, message: {:?})",
                    &now,
                    message.as_ref(),
                )
            })?;

        // Ensure that we query `last_insert_rowid` in a transaction, in case
        // there's another thread in this process making queries with the same
        // SQLite connection.
        let event_tx_id: isize = self.conn.last_insert_rowid().try_into()?;
        tx.commit()?;
        Ok(EventTransactionId(event_tx_id))
    }
}

/// Determine whether a given reference is used to keep a commit alive.
///
/// Args:
/// * `ref_name`: The name of the reference.
///
/// Returns: Whether or not the given reference is used internally to keep the
/// commit alive, so that it's not collected by Git's garbage collection
/// mechanism.
pub fn is_gc_ref(ref_name: &str) -> bool {
    ref_name.starts_with("refs/branchless/")
}

/// Determines whether or not updates to the given reference should be ignored.
///
/// Args:
/// * `ref_name`: The name of the reference to check.
///
/// Returns: Whether or not updates to the given reference should be ignored.
pub fn should_ignore_ref_updates(ref_name: &str) -> bool {
    if is_gc_ref(ref_name) {
        return true;
    }

    matches!(
        ref_name,
        "ORIG_HEAD" | "CHERRY_PICK" | "REBASE_HEAD" | "CHERRY_PICK_HEAD" | "FETCH_HEAD"
    )
}

#[derive(Debug)]
enum EventClassification {
    Show,
    Hide,
}

/// Whether or not a commit is visible.
///
/// This is determined by the last `Event` that affected the commit.
#[derive(Debug)]
pub enum CommitVisibility {
    /// The commit is visible, and should be rendered as part of the commit graph.
    Visible,

    /// The commit is hidden, and should be hidden from the commit graph (unless
    /// a descendant commit is visible).
    Hidden,
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventCursor {
    event_id: isize,
}

/// Processes events in order and determine the repo's visible commits.
#[derive(Debug)]
pub struct EventReplayer {
    /// Events are numbered starting from zero.
    id_counter: isize,

    /// The list of observed events.
    events: Vec<Event>,

    /// The events that have affected each commit.
    commit_history: HashMap<git2::Oid, Vec<EventInfo>>,

    /// Map from ref names to ref locations (an OID or another ref name). Works
    /// around https://github.com/arxanas/git-branchless/issues/7.
    ///
    /// If an entry is not present, it was either never observed, or it most
    /// recently changed to point to the zero hash (i.e. it was deleted).
    ref_locations: HashMap<String, String>,
}

impl EventReplayer {
    fn new() -> Self {
        EventReplayer {
            id_counter: 0,
            events: vec![],
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
    pub fn from_event_log_db(event_log_db: &EventLogDb) -> anyhow::Result<Self> {
        let mut result = EventReplayer::new();
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
            if should_ignore_ref_updates(&ref_name) {
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
                self.commit_history
                    .entry(*old_commit_oid)
                    .or_insert_with(Vec::new)
                    .push(EventInfo {
                        id,
                        event: event.clone(),
                        event_classification: EventClassification::Hide,
                    });
                self.commit_history
                    .entry(*new_commit_oid)
                    .or_insert_with(Vec::new)
                    .push(EventInfo {
                        id,
                        event: event.clone(),
                        event_classification: EventClassification::Show,
                    });
            }

            // A reference update doesn't indicate a change to a commit, so we
            // don't include it in the `commit_history`. We'll traverse the
            // history later to find historical locations of references when
            // needed.
            Event::RefUpdateEvent {
                ref_name, new_ref, ..
            } => match new_ref {
                Some(new_ref) => {
                    self.ref_locations.insert(ref_name.clone(), new_ref.clone());
                }
                None => {
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
                .or_insert_with(Vec::new)
                .push(EventInfo {
                    id,
                    event: event.clone(),
                    event_classification: EventClassification::Show,
                }),

            Event::HideEvent {
                timestamp: _,
                event_tx_id: _,
                commit_oid,
            } => self
                .commit_history
                .entry(*commit_oid)
                .or_insert_with(Vec::new)
                .push(EventInfo {
                    id,
                    event: event.clone(),
                    event_classification: EventClassification::Hide,
                }),

            Event::UnhideEvent {
                timestamp: _,
                event_tx_id: _,
                commit_oid,
            } => self
                .commit_history
                .entry(*commit_oid)
                .or_insert_with(Vec::new)
                .push(EventInfo {
                    id,
                    event: event.clone(),
                    event_classification: EventClassification::Show,
                }),
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
                old_ref: None,
                new_ref: None,
                message,
            } => {
                let old_ref = self.ref_locations.get(&ref_name).cloned();
                Event::RefUpdateEvent {
                    timestamp,
                    event_tx_id,
                    ref_name,
                    old_ref,
                    new_ref: None,
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
                    old_ref: _,
                    new_ref: None,
                    ref message,
                },
                Some(Event::RefUpdateEvent {
                    timestamp: _,
                    event_tx_id: _,
                    ref_name: last_ref_name,
                    old_ref: _,
                    new_ref: None,
                    message: last_message,
                }),
            ) if ref_name == last_ref_name && message == last_message => None,

            (event, _) => Some(event),
        }
    }

    fn get_cursor_commit_history(&self, cursor: EventCursor, oid: git2::Oid) -> Vec<&EventInfo> {
        match self.commit_history.get(&oid) {
            None => vec![],
            Some(history) => history
                .iter()
                .filter(|event_info| event_info.id < cursor.event_id)
                .collect(),
        }
    }

    /// Determines whether a commit has been marked as visible or hidden at the
    /// cursor's point in time.
    ///
    /// Args:
    /// * `oid`: The OID of the commit to check.
    ///
    /// Returns: Whether the commit is visible or hidden. Returns `None` if no
    /// history has been recorded for that commit.
    pub fn get_cursor_commit_visibility(
        &self,
        cursor: EventCursor,
        oid: git2::Oid,
    ) -> Option<CommitVisibility> {
        let history = self.get_cursor_commit_history(cursor, oid);
        let event_info = *history.last()?;
        match event_info.event_classification {
            EventClassification::Show => Some(CommitVisibility::Visible),
            EventClassification::Hide => Some(CommitVisibility::Hidden),
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
        oid: git2::Oid,
    ) -> Option<&Event> {
        let history = self.get_cursor_commit_history(cursor, oid);
        let event_info = *history.last()?;
        Some(&event_info.event)
    }

    /// Get the OIDs which have activity according to the repository history.
    ///
    /// Returns: The set of OIDs referring to commits which are thought to be
    /// active due to user action.
    pub fn get_cursor_active_oids(&self, cursor: EventCursor) -> HashSet<git2::Oid> {
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
    pub fn get_cursor_head_oid(&self, cursor: EventCursor) -> Option<git2::Oid> {
        let cursor_event_id: usize = cursor.event_id.try_into().unwrap();
        self.events[0..cursor_event_id]
            .iter()
            .rev()
            .find_map(|event| {
                match &event {
                    Event::RefUpdateEvent {
                        ref_name,
                        new_ref: Some(new_ref),
                        ..
                    } if ref_name == "HEAD" => match git2::Oid::from_str(&new_ref) {
                        Ok(oid) => Some(oid),
                        Err(_) => {
                            warn!(
                                "Expected HEAD new_ref to point to an OID; \
                                instead pointed to: {:?}",
                                new_ref
                            );
                            None
                        }
                    },
                    Event::RefUpdateEvent { .. } => None,

                    // Not strictly necessary, but helps to compensate in case
                    // the user is not running Git v2.29 or above, and therefore
                    // doesn't have the corresponding `RefUpdateEvent`.
                    Event::CommitEvent { commit_oid, .. } => Some(*commit_oid),

                    Event::RewriteEvent { .. }
                    | Event::HideEvent { .. }
                    | Event::UnhideEvent { .. } => None,
                }
            })
    }

    fn get_cursor_branch_oid(
        &self,
        cursor: EventCursor,
        branch_name: &str,
    ) -> anyhow::Result<Option<git2::Oid>> {
        let cursor_event_id: usize = cursor.event_id.try_into().unwrap();
        let target_ref_name = format!("refs/heads/{}", branch_name);
        let oid = self.events[0..cursor_event_id]
            .iter()
            .rev()
            .find_map(|event| match &event {
                Event::RefUpdateEvent {
                    ref_name,
                    new_ref: Some(new_ref),
                    ..
                } if *ref_name == target_ref_name => Some(new_ref),
                _ => None,
            });
        match oid {
            Some(oid) => {
                let oid = git2::Oid::from_str(&oid)?;
                Ok(Some(oid))
            }
            None => Ok(None),
        }
    }

    /// Get the OID of the main branch at the cursor's point in
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
    pub fn get_cursor_main_branch_oid(
        &self,
        cursor: EventCursor,
        repo: &Repo,
    ) -> anyhow::Result<git2::Oid> {
        let main_branch_name = get_main_branch_name(&repo)?;
        let main_branch_oid = self.get_cursor_branch_oid(cursor, &main_branch_name)?;
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
    pub fn get_cursor_branch_oid_to_names(
        &self,
        cursor: EventCursor,
        repo: &Repo,
    ) -> anyhow::Result<HashMap<git2::Oid, HashSet<String>>> {
        let mut ref_name_to_oid: HashMap<&String, git2::Oid> = HashMap::new();
        let cursor_event_id: usize = cursor.event_id.try_into().unwrap();
        for event in self.events[..cursor_event_id].iter() {
            match event {
                Event::RefUpdateEvent {
                    new_ref: Some(new_ref),
                    ref_name,
                    ..
                } => {
                    if let Ok(oid) = git2::Oid::from_str(new_ref) {
                        ref_name_to_oid.insert(ref_name, oid);
                    }
                }
                Event::RefUpdateEvent {
                    new_ref: None,
                    ref_name,
                    ..
                } => {
                    ref_name_to_oid.remove(ref_name);
                }
                _ => {}
            }
        }

        let mut result: HashMap<git2::Oid, HashSet<String>> = HashMap::new();
        for (ref_name, ref_oid) in ref_name_to_oid.iter() {
            match ref_name.strip_prefix("refs/heads/") {
                None => {}
                Some(branch_name) => {
                    result
                        .entry(*ref_oid)
                        .or_insert_with(HashSet::new)
                        .insert(String::from(branch_name));
                }
            }
        }

        let main_branch_name = get_main_branch_name(&repo)?;
        let main_branch_oid = self.get_cursor_main_branch_oid(cursor, repo)?;
        result
            .entry(main_branch_oid)
            .or_insert_with(HashSet::new)
            .insert(main_branch_name);
        Ok(result)
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

    /// Make a dummy transaction ID, for testing.
    pub fn make_dummy_transaction_id(id: isize) -> EventTransactionId {
        EventTransactionId(id)
    }

    /// Remove the timestamp for the event, for determinism in testing.
    pub fn redact_event_timestamp(mut event: Event) -> Event {
        match event {
            Event::RewriteEvent {
                ref mut timestamp, ..
            } => *timestamp = 0.0,
            Event::RefUpdateEvent {
                ref mut timestamp, ..
            } => *timestamp = 0.0,
            Event::CommitEvent {
                ref mut timestamp, ..
            } => *timestamp = 0.0,
            Event::HideEvent {
                ref mut timestamp, ..
            } => *timestamp = 0.0,
            Event::UnhideEvent {
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::make_git;
    use crate::util::get_db_conn;
    use testing::make_dummy_transaction_id;

    #[test]
    fn test_drop_non_meaningful_events() -> anyhow::Result<()> {
        let event_tx_id = make_dummy_transaction_id(123);
        let meaningful_event = Event::CommitEvent {
            timestamp: 0.0,
            event_tx_id,
            commit_oid: git2::Oid::from_str("abc")?,
        };
        let mut replayer = EventReplayer::new();
        replayer.process_event(&meaningful_event);
        replayer.process_event(&Event::RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id,
            ref_name: String::from("ORIG_HEAD"),
            old_ref: Some(String::from("abc")),
            new_ref: Some(String::from("def")),
            message: None,
        });
        replayer.process_event(&Event::RefUpdateEvent {
            timestamp: 0.0,
            event_tx_id,
            ref_name: String::from("CHERRY_PICK_HEAD"),
            old_ref: None,
            new_ref: None,
            message: None,
        });

        let cursor = replayer.make_default_cursor();
        assert_eq!(
            replayer.get_event_before_cursor(cursor),
            Some((1, &meaningful_event))
        );
        Ok(())
    }

    #[test]
    fn test_different_event_transaction_ids() -> anyhow::Result<()> {
        let git = make_git()?;

        git.init_repo()?;
        git.commit_file("test1", 1)?;
        git.run(&["hide", "HEAD"])?;

        let repo = git.get_repo()?;
        let conn = get_db_conn(&repo)?;
        let event_log_db = EventLogDb::new(&conn)?;
        let events = event_log_db.get_events()?;
        let event_tx_ids: Vec<EventTransactionId> =
            events.iter().map(|event| event.get_event_tx_id()).collect();
        if git.supports_reference_transactions()? {
            insta::assert_debug_snapshot!(event_tx_ids, @r###"
                [
                    EventTransactionId(
                        1,
                    ),
                    EventTransactionId(
                        1,
                    ),
                    EventTransactionId(
                        2,
                    ),
                    EventTransactionId(
                        3,
                    ),
                ]
                "###);
        } else {
            insta::assert_debug_snapshot!(event_tx_ids, @r###"
                [
                    EventTransactionId(
                        1,
                    ),
                    EventTransactionId(
                        2,
                    ),
                ]
                "###);
        }
        Ok(())
    }

    #[test]
    fn test_advance_cursor_by_transaction() -> anyhow::Result<()> {
        let mut event_replayer = EventReplayer::new();
        for (timestamp, event_tx_id) in (0..).zip(&[1, 1, 2, 2, 3, 4]) {
            let timestamp: f64 = timestamp.try_into()?;
            event_replayer.process_event(&Event::UnhideEvent {
                timestamp,
                event_tx_id: EventTransactionId(*event_tx_id),
                commit_oid: git2::Oid::zero(),
            });
        }

        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 0 }, 1),
            EventCursor { event_id: 2 },
        );
        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 1 }, 1),
            EventCursor { event_id: 4 },
        );
        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 2 }, 1),
            EventCursor { event_id: 4 },
        );
        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 3 }, 1),
            EventCursor { event_id: 5 },
        );
        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 4 }, 1),
            EventCursor { event_id: 5 },
        );
        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 5 }, 1),
            EventCursor { event_id: 6 },
        );
        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 6 }, 1),
            EventCursor { event_id: 6 },
        );

        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 6 }, -1),
            EventCursor { event_id: 5 },
        );
        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 5 }, -1),
            EventCursor { event_id: 4 },
        );
        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 4 }, -1),
            EventCursor { event_id: 2 },
        );
        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 3 }, -1),
            EventCursor { event_id: 2 },
        );
        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 2 }, -1),
            EventCursor { event_id: 0 },
        );
        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 1 }, -1),
            EventCursor { event_id: 0 },
        );
        assert_eq!(
            event_replayer.advance_cursor_by_transaction(EventCursor { event_id: 0 }, -1),
            EventCursor { event_id: 0 },
        );

        Ok(())
    }
}
