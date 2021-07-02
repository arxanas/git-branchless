use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

/// Path to the `git` executable on disk to be executed.
#[derive(Clone, Debug)]
pub struct GitRunInfo {
    /// The path to the Git executable on disk.
    pub path_to_git: PathBuf,

    /// The working directory that the Git executable should be run in.
    pub working_directory: PathBuf,

    /// The environment variables that should be passed to the Git process.
    pub env: HashMap<OsString, OsString>,
}
