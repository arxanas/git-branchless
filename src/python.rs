//! Helpers for the Rust/Python interop.
use std::fmt::Debug;

use anyhow::Context;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::PyResult;
use pyo3::types::PyTuple;
use pyo3::{FromPyObject, IntoPy, PyAny, PyObject, Python, ToPyObject};
use rusqlite::NO_PARAMS;

pub fn raise_runtime_error<T>(message: String) -> PyResult<T> {
    Err(PyRuntimeError::new_err(message))
}

pub fn map_err_to_py_err<T, E: Debug, S: AsRef<str>>(
    result: Result<T, E>,
    message: S,
) -> PyResult<T> {
    match result {
        Ok(value) => Ok(value),
        Err(err) => raise_runtime_error(format!(
            "Message: {}\nError details: {:?}",
            message.as_ref(),
            err
        )),
    }
}

/// HACK: Open a new SQLite connection to the same database.
///
/// This is for migration use only, because the connection cannot be shared
/// safely between threads. This is only a concern for accessing the connection
/// from Python. This function should be deleted once we no longer call from
/// Python.
pub fn clone_conn(conn: &rusqlite::Connection) -> anyhow::Result<rusqlite::Connection> {
    let db_path = conn
        .query_row("PRAGMA database_list", NO_PARAMS, |row| {
            let db_path: String = row.get(2)?;
            Ok(db_path)
        })
        .with_context(|| "Querying database list for cloning SQLite database")?;
    let conn = rusqlite::Connection::open(db_path)
        .with_context(|| "Opening cloned database connection")?;
    Ok(conn)
}

pub fn make_conn_from_py_conn(py: Python, conn: PyObject) -> PyResult<rusqlite::Connection> {
    // https://stackoverflow.com/a/14505973
    let query_result =
        conn.call_method1(py, "execute", PyTuple::new(py, &["PRAGMA database_list;"]))?;
    let rows: Vec<(i64, String, String)> =
        query_result.call_method0(py, "fetchall")?.extract(py)?;
    let db_path = match rows.as_slice() {
        [(_, _, path)] => path,
        _ => {
            return Err(PyRuntimeError::new_err(
                "Could not process response from query: PRAGMA database_list",
            ))
        }
    };
    let conn = rusqlite::Connection::open(db_path);
    map_err_to_py_err(
        conn,
        format!("Could not open SQLite database at path {}", db_path),
    )
}

pub fn make_repo_from_py_repo(py: Python, repo: &PyObject) -> PyResult<git2::Repository> {
    let repo_path: String = repo.getattr(py, "path")?.extract(py)?;
    let repo = git2::Repository::open(repo_path);
    let repo = map_err_to_py_err(repo, String::from("Could not open Git repo"))?;
    Ok(repo)
}

pub struct PyOid(pub git2::Oid);

impl<'source> FromPyObject<'source> for PyOid {
    fn extract(obj: &'source PyAny) -> PyResult<Self> {
        let oid: String = obj.getattr("hex")?.extract()?;
        let oid = git2::Oid::from_str(&oid);
        let oid = map_err_to_py_err(oid, "Could not process OID")?;
        Ok(PyOid(oid))
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct PyOidStr(pub git2::Oid);

impl PyOidStr {
    pub fn to_pygit2_oid(&self, py: Python, py_repo: &PyObject) -> PyResult<PyObject> {
        // Convert the OID string into a `pygit2.Oid` object, by calling
        // `repo[oid]` on the Python `repo` object.
        let args = PyTuple::new(py, &[self.0.to_string()]);
        let commit = py_repo.call_method1(py, "__getitem__", args)?;
        let oid = commit.getattr(py, "oid")?;
        Ok(oid)
    }
}

impl<'source> FromPyObject<'source> for PyOidStr {
    fn extract(obj: &'source PyAny) -> PyResult<Self> {
        let oid: String = obj.extract()?;
        let oid = map_err_to_py_err(
            git2::Oid::from_str(&oid),
            format!("Could not process OID: {}", oid),
        )?;
        Ok(PyOidStr(oid))
    }
}

impl ToPyObject for PyOidStr {
    fn to_object(&self, py: Python) -> PyObject {
        self.0.to_string().into_py(py)
    }
}

impl IntoPy<PyObject> for PyOidStr {
    fn into_py(self, py: Python) -> PyObject {
        self.to_object(py)
    }
}

pub struct TextIO<'py> {
    py: Python<'py>,
    text_io: PyObject,
}

impl<'py> TextIO<'py> {
    pub fn new(py: Python<'py>, py_text_io: PyObject) -> TextIO<'py> {
        TextIO {
            py,
            text_io: py_text_io,
        }
    }
}

impl std::io::Write for TextIO<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let buf = String::from_utf8(buf.into()).expect("Could not convert bytes to UTF-8");
        let result = self
            .text_io
            .call_method1(self.py, "write", PyTuple::new(self.py, &[&buf]));
        match result {
            Ok(_) => Ok(buf.len()),
            Err(err) => Err(std::io::Error::new(std::io::ErrorKind::Other, err)),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let result = self.text_io.call_method0(self.py, "flush");
        match result {
            Ok(_) => Ok(()),
            Err(err) => Err(std::io::Error::new(std::io::ErrorKind::Other, err)),
        }
    }
}
