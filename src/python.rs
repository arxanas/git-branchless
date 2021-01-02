//! Helpers for the Rust/Python interop.
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::PyResult;

pub fn map_err_to_py_err<T, E>(result: Result<T, E>, message: &'static str) -> PyResult<T> {
    match result {
        Ok(value) => Ok(value),
        Err(_err) => Err(PyRuntimeError::new_err(message)),
    }
}
