//! Utilities to control output and render to the terminal.

mod cursive;
mod output;

pub use self::cursive::testing;
pub use self::cursive::{with_siv, SingletonView};
pub use output::{OperationType, Output};
