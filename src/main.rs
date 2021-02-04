use std::process;

use branchless::python::raise_runtime_error;
use pyo3::prelude::*;

fn main() -> PyResult<()> {
    Python::with_gil(|py| {
        let sys = PyModule::import(py, "sys")?;
        sys.setattr("argv", std::env::args().collect::<Vec<_>>())?;

        // HACK: assume that the `git-branchless` executable is built with Cargo
        // and installed alongside the Python module `branchless`.
        let mut python_source_root = std::env::current_exe()
            .or_else(|_err| raise_runtime_error(String::from("Could not get current_exe")))?;

        // Should be a path like
        // /path/to/git-branchless/target/debug/git-branchless, so remove the
        // last three components.
        python_source_root.pop();
        python_source_root.pop();
        python_source_root.pop();
        let python_source_root = python_source_root.to_str().map(Ok).unwrap_or_else(|| {
            raise_runtime_error(String::from(
                "Could not convert Python source root to string",
            ))
        })?;
        sys.getattr("path")?
            .call_method1("insert", (0, python_source_root))?;

        let branchless = PyModule::import(py, "branchless.__main__")?;
        match branchless.call0("entry_point") {
            Ok(_) => Ok(()),
            Err(err) => {
                if err.is_instance::<pyo3::exceptions::PySystemExit>(py) {
                    let exit_code = err.into_py(py).getattr(py, "code")?.extract(py)?;
                    process::exit(exit_code)
                } else {
                    err.print(py);
                    Err(err)
                }
            }
        }
    })
}
