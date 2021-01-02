use pyo3::prelude::*;

mod mergebase;

#[pymodule]
fn rust(_py: Python<'_>, module: &PyModule) -> PyResult<()> {
    module.add_class::<mergebase::PyMergeBaseDb>()?;
    Ok(())
}
