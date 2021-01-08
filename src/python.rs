//! Helpers for the Rust/Python interop.
use std::fmt::Debug;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::PyResult;
use pyo3::types::PyTuple;
use pyo3::{FromPyObject, IntoPy, PyAny, PyObject, Python, ToPyObject};

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

#[derive(Clone, Hash)]
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

impl PartialEq for PyOidStr {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for PyOidStr {}

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
