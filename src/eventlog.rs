//! Process our event log.
//!
//! We use Git hooks to record the actions that the user takes over time, and put
//! them in persistent storage. Later, we play back the actions in order to
//! determine what actions the user took on the repository, and which commits
//! they're still working on.
use anyhow::Context;
use pyo3::prelude::*;

use crate::python::{get_conn, map_err_to_py_err, raise_runtime_error};
use crate::util::wrap_git_error;

/// Wrapper around the row stored directly in the database.
struct Row {
    timestamp: f64,
    type_: String,
    ref1: Option<String>,
    ref2: Option<String>,
    ref_name: Option<String>,
    message: Option<String>,
}

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

struct EventLogDb {
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

#[derive(Clone)]
struct PyGitOid(git2::Oid);

impl<'source> FromPyObject<'source> for PyGitOid {
    fn extract(obj: &'source PyAny) -> PyResult<Self> {
        let oid: String = obj.extract()?;
        let oid = map_err_to_py_err(
            git2::Oid::from_str(&oid),
            format!("Could not process OID: {}", oid),
        )?;
        Ok(PyGitOid(oid))
    }
}

impl IntoPy<PyObject> for PyGitOid {
    fn into_py(self, py: Python) -> PyObject {
        self.0.to_string().into_py(py)
    }
}

#[pyclass]
#[derive(FromPyObject)]
#[allow(dead_code)]
pub struct PyRewriteEvent {
    #[pyo3(get)]
    timestamp: f64,

    #[pyo3(get)]
    old_commit_oid: PyGitOid,

    #[pyo3(get)]
    new_commit_oid: PyGitOid,
}

#[pymethods]
impl PyRewriteEvent {
    #[new]
    fn new(timestamp: f64, old_commit_oid: PyGitOid, new_commit_oid: PyGitOid) -> Self {
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
    commit_oid: PyGitOid,
}

#[pymethods]
impl PyCommitEvent {
    #[new]
    fn new(timestamp: f64, commit_oid: PyGitOid) -> Self {
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
    commit_oid: PyGitOid,
}

#[pymethods]
impl PyHideEvent {
    #[new]
    fn new(timestamp: f64, commit_oid: PyGitOid) -> Self {
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
    commit_oid: PyGitOid,
}

#[pymethods]
impl PyUnhideEvent {
    #[new]
    fn new(timestamp: f64, commit_oid: PyGitOid) -> Self {
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
            .map(|event: &PyObject| {
                let event_type: String = event.getattr(py, "type")?.extract(py)?;

                let event = match event_type.as_str() {
                    "rewrite" => {
                        let PyRewriteEvent {
                            timestamp,
                            old_commit_oid: PyGitOid(old_commit_oid),
                            new_commit_oid: PyGitOid(new_commit_oid),
                        } = event.into_py(py).extract(py)?;
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
                        } = event.into_py(py).extract(py)?;
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
                            commit_oid: PyGitOid(commit_oid),
                        } = event.into_py(py).extract(py)?;
                        Event::CommitEvent {
                            timestamp,
                            commit_oid,
                        }
                    }

                    "hide" => {
                        let PyHideEvent {
                            timestamp,
                            commit_oid: PyGitOid(commit_oid),
                        } = event.into_py(py).extract(py)?;
                        Event::HideEvent {
                            timestamp,
                            commit_oid,
                        }
                    }

                    "unhide" => {
                        let PyUnhideEvent {
                            timestamp,
                            commit_oid: PyGitOid(commit_oid),
                        } = event.into_py(py).extract(py)?;
                        Event::UnhideEvent {
                            timestamp,
                            commit_oid,
                        }
                    }

                    other => raise_runtime_error(format!("Unknown event type: {}", other))?,
                };
                Ok(event)
            })
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
                        old_commit_oid: PyGitOid(*old_commit_oid),
                        new_commit_oid: PyGitOid(*new_commit_oid),
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
                        commit_oid: PyGitOid(*commit_oid),
                    };
                    py_event.into_py(py)
                }

                Event::HideEvent {
                    timestamp,
                    commit_oid,
                } => {
                    let py_event = PyHideEvent {
                        timestamp: *timestamp,
                        commit_oid: PyGitOid(*commit_oid),
                    };
                    py_event.into_py(py)
                }

                Event::UnhideEvent {
                    timestamp,
                    commit_oid,
                } => {
                    let py_event = PyUnhideEvent {
                        timestamp: *timestamp,
                        commit_oid: PyGitOid(*commit_oid),
                    };
                    py_event.into_py(py)
                }
            })
            .collect();
        Ok(py_events)
    }
}
