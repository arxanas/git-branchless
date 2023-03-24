//! Utility functions.

use std::num::TryFromIntError;
use std::path::PathBuf;
use std::process::ExitStatus;

/// Represents the code to exit the process with.
#[must_use]
#[derive(Copy, Clone, Debug)]
pub struct ExitCode(pub isize);

impl ExitCode {
    /// Return an exit code corresponding to success.
    pub fn success() -> Self {
        Self(0)
    }

    /// Determine whether or not this exit code represents a successful
    /// termination.
    pub fn is_success(&self) -> bool {
        match self {
            ExitCode(0) => true,
            ExitCode(_) => false,
        }
    }
}

impl TryFrom<ExitStatus> for ExitCode {
    type Error = TryFromIntError;

    fn try_from(status: ExitStatus) -> Result<Self, Self::Error> {
        let exit_code = status.code().unwrap_or(1);
        Ok(Self(exit_code.try_into()?))
    }
}

/// Helper type alias for the common case that we want to run a computation and
/// return `eyre::Result<T>`, but it's also possible that we run a subcommand
/// which returns an exit code that we want to propagate. See also `try_exit_code`.
pub type EyreExitOr<T> = eyre::Result<Result<T, ExitCode>>;

/// Macro to propagate `ExitCode`s in the same way as the `try!` macro/the `?`
/// operator.
///
/// Ideally, we would make `ExitCode` implement `std::ops::Try`, but that's a
/// nightly API. We could also make `ExitCode` implement `Error`, but this
/// interacts badly with `eyre::Result`, because all `Error`s are convertible to
/// `eyre::Error`, so our exit codes get treated at the same as other errors.
/// So, instead, we have this macro to accomplish the same thing, but for
/// `Result<T, ExitCode>`s specifically.
#[macro_export]
macro_rules! try_exit_code {
    ($e:expr) => {
        match $e {
            Ok(value) => value,
            Err(exit_code) => {
                return Ok(Err(exit_code));
            }
        }
    };
}

/// Returns a path for a given file, searching through PATH to find it.
pub fn get_from_path(exe_name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let bash_path = dir.join(exe_name);
            if bash_path.is_file() {
                Some(bash_path)
            } else {
                None
            }
        })
    })
}

/// Returns the path to a shell suitable for running hooks.
pub fn get_sh() -> Option<PathBuf> {
    let exe_name = if cfg!(target_os = "windows") {
        "bash.exe"
    } else {
        "sh"
    };
    // If we are on Windows, first look for git.exe, and try to use it's bash, otherwise it won't
    // be able to find git-branchless correctly.
    if cfg!(target_os = "windows") {
        // Git is typically installed at C:\Program Files\Git\cmd\git.exe with the cmd\ directory
        // in the path, however git-bash is usually not in PATH and is in bin\ directory:
        let git_path = get_from_path("git.exe").expect("Couldn't find git.exe");
        let git_dir = git_path.parent().unwrap().parent().unwrap();
        let git_bash = git_dir.join("bin").join(exe_name);
        if git_bash.is_file() {
            return Some(git_bash);
        }
    }
    get_from_path(exe_name)
}
