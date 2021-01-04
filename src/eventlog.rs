//! Process our event log.
//!
//! We use Git hooks to record the actions that the user takes over time, and put
//! them in persistent storage. Later, we play back the actions in order to
//! determine what actions the user took on the repository, and which commits
//! they're still working on.
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;

use anyhow::Context;
use pyo3::prelude::*;
use pyo3::types::PyType;

use crate::config::get_main_branch_name;
use crate::python::{get_conn, get_repo, map_err_to_py_err, raise_runtime_error, PyOidStr};
use crate::util::{get_main_branch_oid, wrap_git_error};

/// Wrapper around the row stored directly in the database.
struct Row {
    timestamp: f64,
    type_: String,
    ref1: Option<String>,
    ref2: Option<String>,
    ref_name: Option<String>,
    message: Option<String>,
}

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

        /// The OID of the commit that was unhidden.
        commit_oid: git2::Oid,
    },
}

impl Event {
    fn to_row(&self) -> Row {
        match self {
            Event::RewriteEvent {
                timestamp,
                old_commit_oid,
                new_commit_oid,
            } => Row {
                timestamp: *timestamp,
                type_: String::from("rewrite"),
                ref1: Some(old_commit_oid.to_string()),
                ref2: Some(new_commit_oid.to_string()),
                ref_name: None,
                message: None,
            },

            Event::RefUpdateEvent {
                timestamp,
                ref_name,
                old_ref,
                new_ref,
                message,
            } => Row {
                timestamp: *timestamp,
                type_: String::from("ref-move"),
                ref1: old_ref.clone(),
                ref2: new_ref.clone(),
                ref_name: Some(ref_name.clone()),
                message: message.clone(),
            },

            Event::CommitEvent {
                timestamp,
                commit_oid,
            } => Row {
                timestamp: *timestamp,
                type_: String::from("commit"),
                ref1: Some(commit_oid.to_string()),
                ref2: None,
                ref_name: None,
                message: None,
            },

            Event::HideEvent {
                timestamp,
                commit_oid,
            } => Row {
                timestamp: *timestamp,
                type_: String::from("hide"),
                ref1: Some(commit_oid.to_string()),
                ref2: None,
                ref_name: None,
                message: None,
            },

            Event::UnhideEvent {
                timestamp,
                commit_oid,
            } => Row {
                timestamp: *timestamp,
                type_: String::from("unhide"),
                ref1: Some(commit_oid.to_string()),
                ref2: None,
                ref_name: None,
                message: None,
            },
        }
    }

    fn from_row(row: &Row) -> anyhow::Result<Self> {
        let Row {
            timestamp,
            type_,
            ref_name,
            ref1,
            ref2,
            message,
        } = row;

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
                let old_commit_oid = get_oid(ref1, "old commit OID")?;
                let new_commit_oid = get_oid(ref2, "new commit OID")?;
                Event::RewriteEvent {
                    timestamp: *timestamp,
                    old_commit_oid,
                    new_commit_oid,
                }
            }

            "ref-move" => {
                let ref_name = ref_name
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("ref-move event missing ref name"))?;
                Event::RefUpdateEvent {
                    timestamp: *timestamp,
                    ref_name,
                    old_ref: ref1.clone(),
                    new_ref: ref2.clone(),
                    message: message.clone(),
                }
            }

            "commit" => {
                let commit_oid = get_oid(ref1, "commit OID")?;
                Event::CommitEvent {
                    timestamp: *timestamp,
                    commit_oid,
                }
            }

            "hide" => {
                let commit_oid = get_oid(ref1, "commit OID")?;
                Event::HideEvent {
                    timestamp: *timestamp,
                    commit_oid,
                }
            }

            "unhide" => {
                let commit_oid = get_oid(ref1, "commit OID")?;
                Event::UnhideEvent {
                    timestamp: *timestamp,
                    commit_oid,
                }
            }

            other => anyhow::bail!("Unknown event type {}", other),
        };
        Ok(event)
    }
}

pub struct EventLogDb {
    conn: rusqlite::Connection,
}

fn init_tables(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    conn.execute(
        "
CREATE TABLE IF NOT EXISTS event_log (
    timestamp REAL NOT NULL,
    type TEXT NOT NULL,
    old_ref TEXT,
    new_ref TEXT,
    ref_name TEXT,
    message TEXT
)
",
        rusqlite::params![],
    )
    .context("Creating tables")?;
    Ok(())
}

/// Stores `Event`s on disk.
impl EventLogDb {
    fn new(conn: rusqlite::Connection) -> anyhow::Result<Self> {
        init_tables(&conn)?;
        Ok(EventLogDb { conn })
    }

    /// Add events in the given order to the database, in a transaction.
    ///
    /// Args:
    /// * events: The events to add.
    fn add_events(&mut self, events: Vec<Event>) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        for event in events {
            let row = event.to_row();
            tx.execute_named(
                "
INSERT INTO event_log VALUES (
    :timestamp,
    :type,
    :old_ref,
    :new_ref,
    :ref_name,
    :message
)
            ",
                rusqlite::named_params! {
                    ":timestamp": row.timestamp,
                    ":type": &row.type_,
                    ":old_ref": &row.ref1,
                    ":new_ref": &row.ref2,
                    ":ref_name": &row.ref_name,
                    ":message": &row.message,
                },
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Get all the events in the database.
    ///
    /// Returns: All the events in the database, ordered from oldest to newest.
    fn get_events(&self) -> anyhow::Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "
SELECT timestamp, type, old_ref, new_ref, ref_name, message
FROM event_log
ORDER BY rowid ASC
",
        )?;
        let rows: rusqlite::Result<Vec<Row>> = stmt
            .query_map(rusqlite::params![], |row| {
                let timestamp: f64 = row.get("timestamp")?;
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
                    type_,
                    ref_name,
                    ref1: old_ref,
                    ref2: new_ref,
                    message,
                })
            })?
            .collect();
        let rows = rows?;
        let events = rows.iter().map(|row| Event::from_row(row)).collect();
        events
    }
}

#[pyclass]
#[derive(FromPyObject)]
#[allow(dead_code)]
pub struct PyRewriteEvent {
    #[pyo3(get)]
    timestamp: f64,

    #[pyo3(get)]
    old_commit_oid: PyOidStr,

    #[pyo3(get)]
    new_commit_oid: PyOidStr,
}

#[pymethods]
impl PyRewriteEvent {
    #[new]
    fn new(timestamp: f64, old_commit_oid: PyOidStr, new_commit_oid: PyOidStr) -> Self {
        Self {
            timestamp,
            old_commit_oid,
            new_commit_oid,
        }
    }

    #[getter]
    fn get_type(&self) -> String {
        String::from("rewrite")
    }
}

#[pyclass]
#[derive(FromPyObject)]
#[allow(dead_code)]
pub struct PyRefUpdateEvent {
    #[pyo3(get)]
    timestamp: f64,

    #[pyo3(get)]
    ref_name: String,

    #[pyo3(get)]
    old_ref: Option<String>,

    #[pyo3(get)]
    new_ref: Option<String>,

    #[pyo3(get)]
    message: Option<String>,
}

#[pymethods]
impl PyRefUpdateEvent {
    #[new]
    fn new(
        timestamp: f64,
        ref_name: String,
        old_ref: Option<String>,
        new_ref: Option<String>,
        message: Option<String>,
    ) -> Self {
        Self {
            timestamp,
            ref_name,
            old_ref,
            new_ref,
            message,
        }
    }

    #[getter]
    fn get_type(&self) -> String {
        String::from("ref-move")
    }
}

#[pyclass]
#[derive(FromPyObject)]
#[allow(dead_code)]
pub struct PyCommitEvent {
    #[pyo3(get)]
    timestamp: f64,

    #[pyo3(get)]
    commit_oid: PyOidStr,
}

#[pymethods]
impl PyCommitEvent {
    #[new]
    fn new(timestamp: f64, commit_oid: PyOidStr) -> Self {
        Self {
            timestamp,
            commit_oid,
        }
    }

    #[getter]
    fn get_type(&self) -> String {
        String::from("commit")
    }
}

#[pyclass]
#[derive(FromPyObject)]
#[allow(dead_code)]
pub struct PyHideEvent {
    #[pyo3(get)]
    timestamp: f64,

    #[pyo3(get)]
    commit_oid: PyOidStr,
}

#[pymethods]
impl PyHideEvent {
    #[new]
    fn new(timestamp: f64, commit_oid: PyOidStr) -> Self {
        Self {
            timestamp,
            commit_oid,
        }
    }

    #[getter]
    fn get_type(&self) -> String {
        String::from("hide")
    }
}

#[pyclass]
#[derive(FromPyObject)]
#[allow(dead_code)]
pub struct PyUnhideEvent {
    #[pyo3(get)]
    timestamp: f64,

    #[pyo3(get)]
    commit_oid: PyOidStr,
}

#[pymethods]
impl PyUnhideEvent {
    #[new]
    fn new(timestamp: f64, commit_oid: PyOidStr) -> Self {
        Self {
            timestamp,
            commit_oid,
        }
    }

    #[getter]
    fn get_type(&self) -> String {
        String::from("unhide")
    }
}

impl IntoPy<PyObject> for Event {
    fn into_py(self, py: Python) -> PyObject {
        match self {
            Event::RewriteEvent {
                timestamp,
                old_commit_oid,
                new_commit_oid,
            } => PyRewriteEvent {
                timestamp,
                old_commit_oid: PyOidStr(old_commit_oid),
                new_commit_oid: PyOidStr(new_commit_oid),
            }
            .into_py(py),

            Event::RefUpdateEvent {
                timestamp,
                ref_name,
                old_ref,
                new_ref,
                message,
            } => PyRefUpdateEvent {
                timestamp,
                ref_name,
                old_ref,
                new_ref,
                message,
            }
            .into_py(py),

            Event::CommitEvent {
                timestamp,
                commit_oid,
            } => PyCommitEvent {
                timestamp,
                commit_oid: PyOidStr(commit_oid),
            }
            .into_py(py),

            Event::HideEvent {
                timestamp,
                commit_oid,
            } => PyHideEvent {
                timestamp,
                commit_oid: PyOidStr(commit_oid),
            }
            .into_py(py),

            Event::UnhideEvent {
                timestamp,
                commit_oid,
            } => PyUnhideEvent {
                timestamp,
                commit_oid: PyOidStr(commit_oid),
            }
            .into_py(py),
        }
    }
}

fn py_event_to_event(py: Python, py_event: &PyObject) -> PyResult<Event> {
    let event_type: String = py_event.getattr(py, "type")?.extract(py)?;
    let event = match event_type.as_str() {
        "rewrite" => {
            let PyRewriteEvent {
                timestamp,
                old_commit_oid: PyOidStr(old_commit_oid),
                new_commit_oid: PyOidStr(new_commit_oid),
            } = py_event.into_py(py).extract(py)?;
            Event::RewriteEvent {
                timestamp,
                old_commit_oid,
                new_commit_oid,
            }
        }

        "ref-move" => {
            let PyRefUpdateEvent {
                timestamp,
                ref_name,
                old_ref,
                new_ref,
                message,
            } = py_event.into_py(py).extract(py)?;
            Event::RefUpdateEvent {
                timestamp,
                ref_name,
                old_ref,
                new_ref,
                message,
            }
        }

        "commit" => {
            let PyCommitEvent {
                timestamp,
                commit_oid: PyOidStr(commit_oid),
            } = py_event.into_py(py).extract(py)?;
            Event::CommitEvent {
                timestamp,
                commit_oid,
            }
        }

        "hide" => {
            let PyHideEvent {
                timestamp,
                commit_oid: PyOidStr(commit_oid),
            } = py_event.into_py(py).extract(py)?;
            Event::HideEvent {
                timestamp,
                commit_oid,
            }
        }

        "unhide" => {
            let PyUnhideEvent {
                timestamp,
                commit_oid: PyOidStr(commit_oid),
            } = py_event.into_py(py).extract(py)?;
            Event::UnhideEvent {
                timestamp,
                commit_oid,
            }
        }

        other => raise_runtime_error(format!("Unknown event type: {}", other))?,
    };
    Ok(event)
}

#[pyclass]
pub struct PyEventLogDb {
    event_log_db: EventLogDb,
}

#[pymethods]
impl PyEventLogDb {
    #[new]
    fn new(py: Python, conn: PyObject) -> PyResult<Self> {
        let conn = get_conn(py, conn)?;
        let event_log_db = EventLogDb::new(conn);
        let event_log_db = map_err_to_py_err(
            event_log_db,
            String::from("Could not construct event log database"),
        )?;
        let event_log_db = PyEventLogDb { event_log_db };
        Ok(event_log_db)
    }

    fn add_events(&mut self, py: Python, events: Vec<PyObject>) -> PyResult<()> {
        let events: PyResult<Vec<Event>> = events
            .iter()
            .map(|event| py_event_to_event(py, event))
            .collect();
        let events = map_err_to_py_err(events, String::from("Could not process events"))?;
        let result = self.event_log_db.add_events(events);
        map_err_to_py_err(result, String::from("Could not add events"))
    }

    fn get_events(&self, py: Python) -> PyResult<Vec<PyObject>> {
        let events = self.event_log_db.get_events();
        let events = map_err_to_py_err(events, String::from("Could not get events"))?;
        let py_events = events
            .iter()
            .map(|event| match event {
                Event::RewriteEvent {
                    timestamp,
                    old_commit_oid,
                    new_commit_oid,
                } => {
                    let py_event = PyRewriteEvent {
                        timestamp: *timestamp,
                        old_commit_oid: PyOidStr(*old_commit_oid),
                        new_commit_oid: PyOidStr(*new_commit_oid),
                    };
                    py_event.into_py(py)
                }

                Event::RefUpdateEvent {
                    timestamp,
                    ref_name,
                    old_ref,
                    new_ref,
                    message,
                } => {
                    let py_event = PyRefUpdateEvent {
                        timestamp: *timestamp,
                        ref_name: ref_name.clone(),
                        old_ref: old_ref.clone(),
                        new_ref: new_ref.clone(),
                        message: message.clone(),
                    };
                    py_event.into_py(py)
                }

                Event::CommitEvent {
                    timestamp,
                    commit_oid,
                } => {
                    let py_event = PyCommitEvent {
                        timestamp: *timestamp,
                        commit_oid: PyOidStr(*commit_oid),
                    };
                    py_event.into_py(py)
                }

                Event::HideEvent {
                    timestamp,
                    commit_oid,
                } => {
                    let py_event = PyHideEvent {
                        timestamp: *timestamp,
                        commit_oid: PyOidStr(*commit_oid),
                    };
                    py_event.into_py(py)
                }

                Event::UnhideEvent {
                    timestamp,
                    commit_oid,
                } => {
                    let py_event = PyUnhideEvent {
                        timestamp: *timestamp,
                        commit_oid: PyOidStr(*commit_oid),
                    };
                    py_event.into_py(py)
                }
            })
            .collect();
        Ok(py_events)
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

    match ref_name {
        "ORIG_HEAD" | "CHERRY_PICK" => true,
        _ => false,
    }
}

enum EventClassification {
    Show,
    Hide,
}

pub enum CommitVisibility {
    Visible,
    Hidden,
}

struct EventInfo {
    id: isize,
    event: Event,
    event_classification: EventClassification,
}

pub struct EventReplayer {
    /// Events are numbered starting from zero.
    id_counter: isize,

    /// Events up to this number (exclusive) are available to the caller.
    cursor_event_id: isize,

    /// The list of observed events.
    events: Vec<Event>,

    /// The events that have affected each commit.
    commit_history: HashMap<git2::Oid, Vec<EventInfo>>,
}

/// Processes events in order and determine the repo's visible commits.
impl EventReplayer {
    fn new() -> Self {
        EventReplayer {
            id_counter: 0,
            cursor_event_id: 0,
            events: vec![],
            commit_history: HashMap::new(),
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
        match &event {
            Event::RefUpdateEvent {
                ref_name,
                old_ref,
                new_ref,
                ..
            } => {
                if should_ignore_ref_updates(&ref_name) || (old_ref.is_none() && new_ref.is_none())
                {
                    return;
                }
            }
            _ => (),
        }

        let id = self.id_counter;
        self.id_counter += 1;
        self.cursor_event_id = self.id_counter;
        self.events.push(event.clone());

        match &event {
            Event::RewriteEvent {
                timestamp: _,
                old_commit_oid,
                new_commit_oid,
            } => {
                self.commit_history
                    .entry(*old_commit_oid)
                    .or_insert(vec![])
                    .push(EventInfo {
                        id,
                        event: event.clone(),
                        event_classification: EventClassification::Hide,
                    });
                self.commit_history
                    .entry(*new_commit_oid)
                    .or_insert(vec![])
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
            Event::RefUpdateEvent { .. } => (),

            Event::CommitEvent {
                timestamp: _,
                commit_oid,
            } => self
                .commit_history
                .entry(*commit_oid)
                .or_insert(vec![])
                .push(EventInfo {
                    id,
                    event: event.clone(),
                    event_classification: EventClassification::Show,
                }),

            Event::HideEvent {
                timestamp: _,
                commit_oid,
            } => self
                .commit_history
                .entry(*commit_oid)
                .or_insert(vec![])
                .push(EventInfo {
                    id,
                    event: event.clone(),
                    event_classification: EventClassification::Hide,
                }),

            Event::UnhideEvent {
                timestamp: _,
                commit_oid,
            } => self
                .commit_history
                .entry(*commit_oid)
                .or_insert(vec![])
                .push(EventInfo {
                    id,
                    event: event.clone(),
                    event_classification: EventClassification::Show,
                }),
        };
    }

    fn get_cursor_commit_history(&self, oid: git2::Oid) -> Vec<&EventInfo> {
        match self.commit_history.get(&oid) {
            None => vec![],
            Some(history) => history
                .iter()
                .filter(|event_info| event_info.id < self.cursor_event_id)
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
    pub fn get_cursor_commit_visibility(&self, oid: git2::Oid) -> Option<CommitVisibility> {
        let history = self.get_cursor_commit_history(oid);
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
    pub fn get_cursor_commit_latest_event(&self, oid: git2::Oid) -> Option<&Event> {
        let history = self.get_cursor_commit_history(oid);
        let event_info = *history.last()?;
        Some(&event_info.event)
    }

    /// Get the OIDs which have activity according to the repository history.
    ///
    /// Returns: The set of OIDs referring to commits which are thought to be
    /// active due to user action.
    pub fn get_active_oids(&self) -> HashSet<git2::Oid> {
        self.commit_history
            .iter()
            .filter_map(|(oid, history)| {
                if history.iter().any(|event| event.id < self.cursor_event_id) {
                    Some(*oid)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Set the event cursor to point to immediately after the provided event.
    ///
    /// The "event cursor" is used to move the event replayer forward or
    /// backward in time, so as to show the state of the repository at that
    /// time.
    ///
    /// The cursor is a position in between two events in the event log.
    /// Thus, all events before to the cursor are considered to be in effect,
    /// and all events after the cursor are considered to not have happened
    /// yet.
    ///
    /// Args:
    /// * `event_id`: The index of the event to set the cursor to point
    /// immediately after. If out of bounds, the cursor is set to the first or
    /// last valid position, as appropriate.
    fn set_cursor(&mut self, event_id: isize) {
        let event_id = if event_id < 0 { 0 } else { event_id };
        let num_events: isize = self.events.len().try_into().unwrap();
        let event_id = if event_id > num_events {
            num_events
        } else {
            event_id
        };
        self.cursor_event_id = event_id;
    }

    /// Advance the event cursor by the specified number of events.
    ///
    /// Args:
    /// * `num_events`: The number of events to advance by. Can be positive,
    /// zero, or negative. If out of bounds, the cursor is set to the first or
    /// last valid position, as appropriate.
    fn advance_cursor(&mut self, num_events: isize) {
        self.set_cursor(self.cursor_event_id + num_events)
    }

    /// Get the OID of `HEAD` at the cursor's point in time.
    ///
    /// Returns: The OID pointed to by `HEAD` at that time, or `None` if `HEAD`
    /// was never observed.
    fn get_cursor_head_oid(&self) -> Option<git2::Oid> {
        let cursor_event_id: usize = self.cursor_event_id.try_into().unwrap();
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
                            eprintln!(
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

    fn get_cursor_branch_oid(&self, branch_name: &str) -> anyhow::Result<Option<git2::Oid>> {
        let cursor_event_id: usize = self.cursor_event_id.try_into().unwrap();
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
    pub fn get_cursor_main_branch_oid(&self, repo: &git2::Repository) -> anyhow::Result<git2::Oid> {
        let main_branch_name = get_main_branch_name(&repo)?;
        let main_branch_oid = self.get_cursor_branch_oid(&main_branch_name)?;
        match main_branch_oid {
            Some(main_branch_oid) => Ok(main_branch_oid),
            None => {
                // Assume the main branch just hasn't been observed moving yet,
                // so its value at the current time is fine to use.
                get_main_branch_oid(&repo)
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
        repo: &git2::Repository,
    ) -> anyhow::Result<HashMap<git2::Oid, HashSet<String>>> {
        let mut ref_name_to_oid: HashMap<&String, git2::Oid> = HashMap::new();
        let cursor_event_id: usize = self.cursor_event_id.try_into().unwrap();
        for event in self.events[..cursor_event_id].iter() {
            match event {
                Event::RefUpdateEvent {
                    new_ref: Some(new_ref),
                    ref_name,
                    ..
                } => match git2::Oid::from_str(new_ref) {
                    Ok(oid) => {
                        ref_name_to_oid.insert(ref_name, oid);
                    }
                    Err(_) => {}
                },
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
                        .or_insert_with(|| HashSet::new())
                        .insert(String::from(branch_name));
                }
            }
        }

        let main_branch_name = get_main_branch_name(&repo)?;
        let main_branch_oid = self.get_cursor_main_branch_oid(repo)?;
        result
            .entry(main_branch_oid)
            .or_insert_with(|| HashSet::new())
            .insert(main_branch_name);
        Ok(result)
    }

    /// Get the event immediately before the cursor.
    ///
    /// Returns: A tuple of event ID and the event that most recently happened.
    /// If no event was before the event cursor, returns `None` instead.
    fn get_event_before_cursor(&self) -> Option<(isize, &Event)> {
        if self.cursor_event_id == 0 {
            None
        } else {
            let previous_cursor_event_id: usize = (self.cursor_event_id - 1).try_into().unwrap();
            Some((self.cursor_event_id, &self.events[previous_cursor_event_id]))
        }
    }

    /// Get all the events that have happened since the event cursor.
    ///
    /// Returns: An ordered list of events that have happened since the event
    /// cursor, from least recent to most recent.
    fn get_events_since_cursor(&self) -> &[Event] {
        let cursor_event_id: usize = self.cursor_event_id.try_into().unwrap();
        &self.events[cursor_event_id..]
    }
}

#[pyclass]
pub struct PyEventReplayer {
    event_replayer: EventReplayer,
}

#[pymethods]
impl PyEventReplayer {
    #[new]
    fn new() -> Self {
        let event_replayer = EventReplayer::new();
        PyEventReplayer { event_replayer }
    }

    #[classmethod]
    fn from_event_log_db(_cls: &PyType, py_event_log_db: &PyEventLogDb) -> PyResult<Self> {
        let event_replayer = EventReplayer::from_event_log_db(&py_event_log_db.event_log_db);
        let event_replayer = map_err_to_py_err(
            event_replayer,
            String::from("Could not process event log DB"),
        )?;
        Ok(PyEventReplayer { event_replayer })
    }

    fn process_event(&mut self, py: Python, py_event: PyObject) -> PyResult<()> {
        let event = py_event_to_event(py, &py_event)?;
        self.event_replayer.process_event(&event);
        Ok(())
    }

    pub fn get_cursor_commit_visibility(&self, py: Python, oid: PyOidStr) -> PyResult<PyObject> {
        let oid = oid.0;
        let commit_visibility = self.event_replayer.get_cursor_commit_visibility(oid);
        match commit_visibility {
            Some(CommitVisibility::Visible) => Ok(String::from("visible").into_py(py)),
            Some(CommitVisibility::Hidden) => Ok(String::from("hidden").into_py(py)),
            None => Ok(py.None()),
        }
    }

    fn get_commit_latest_event(&self, py: Python, oid: PyOidStr) -> PyResult<PyObject> {
        let oid = oid.0;
        match self.event_replayer.get_cursor_commit_latest_event(oid) {
            Some(event) => {
                // `into_py` takes `self` instead of `&self`, so we have to
                // clone `event`.
                Ok(event.clone().into_py(py))
            }
            None => Ok(py.None()),
        }
    }

    fn get_active_oids(&self) -> HashSet<String> {
        self.event_replayer
            .get_active_oids()
            .iter()
            .map(|oid| oid.to_string())
            .collect()
    }

    fn set_cursor(&mut self, event_id: isize) {
        self.event_replayer.set_cursor(event_id)
    }

    fn advance_cursor(&mut self, num_events: isize) {
        self.event_replayer.advance_cursor(num_events)
    }

    fn get_cursor_head_oid(&self) -> Option<PyOidStr> {
        self.event_replayer
            .get_cursor_head_oid()
            .map(|oid| PyOidStr(oid))
    }

    fn get_cursor_main_branch_oid(&self, py: Python, repo: PyObject) -> PyResult<PyObject> {
        let py_repo = &repo;
        let repo = get_repo(py, &repo)?;
        let result = self.event_replayer.get_cursor_main_branch_oid(&repo);
        let result =
            map_err_to_py_err(result, String::from("Could not get cursor main branch OID"))?;
        let result = PyOidStr(result);
        let result = result.to_pygit2_oid(py, &py_repo)?;
        Ok(result)
    }

    fn get_cursor_branch_oid_to_names(
        &self,
        py: Python,
        repo: PyObject,
    ) -> PyResult<HashMap<PyOidStr, HashSet<String>>> {
        let repo = get_repo(py, &repo)?;
        let result = self.event_replayer.get_cursor_branch_oid_to_names(&repo);
        let result = map_err_to_py_err(
            result,
            String::from("Could not calculate branch-oid-to-names map"),
        )?;
        let result: HashMap<PyOidStr, HashSet<String>> = result
            .into_iter()
            .map(|(key, value)| (PyOidStr(key), value))
            .collect();
        Ok(result)
    }

    fn get_event_before_cursor(&self, py: Python) -> Option<(isize, PyObject)> {
        self.event_replayer
            .get_event_before_cursor()
            .map(|(id, event)| (id, event.clone().into_py(py)))
    }

    fn get_events_since_cursor(&self, py: Python) -> Vec<PyObject> {
        self.event_replayer
            .get_events_since_cursor()
            .iter()
            .map(|event| event.clone().into_py(py))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drop_non_meaningful_events() -> anyhow::Result<()> {
        let meaningful_event = Event::CommitEvent {
            timestamp: 0.0,
            commit_oid: git2::Oid::from_str("abc")?,
        };
        let mut replayer = EventReplayer::new();
        replayer.process_event(&meaningful_event);
        replayer.process_event(&Event::RefUpdateEvent {
            timestamp: 0.0,
            ref_name: String::from("ORIG_HEAD"),
            old_ref: Some(String::from("abc")),
            new_ref: Some(String::from("def")),
            message: None,
        });
        replayer.process_event(&Event::RefUpdateEvent {
            timestamp: 0.0,
            ref_name: String::from("CHERRY_PICK_HEAD"),
            old_ref: None,
            new_ref: None,
            message: None,
        });

        assert_eq!(
            replayer.get_event_before_cursor(),
            Some((1, &meaningful_event))
        );
        Ok(())
    }
}
