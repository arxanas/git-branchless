//! Helpers for the Rust/Python interop.
use std::fmt::Debug;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::PyResult;
use pyo3::types::PyTuple;
use pyo3::{PyObject, Python};

pub fn raise_runtime_error<T>(message: String) -> PyResult<T> {
    Err(PyRuntimeError::new_err(message))
}

pub fn map_err_to_py_err<T, E: Debug>(result: Result<T, E>, message: String) -> PyResult<T> {
    match result {
        Ok(value) => Ok(value),
        Err(err) => raise_runtime_error(format!("Message: {}\nError details: {:?}", message, err)),
    }
}

pub fn get_conn(py: Python, conn: PyObject) -> PyResult<rusqlite::Connection> {
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
