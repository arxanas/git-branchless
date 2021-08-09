//! Utilities to control output and render to the terminal.

mod cursive;
mod effects;

pub use self::cursive::testing;
pub use self::cursive::{with_siv, SingletonView};
pub use effects::{Effects, OperationType};
