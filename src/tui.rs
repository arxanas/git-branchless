//! Utilities to control output and render to the terminal.

mod cursive;

pub use self::cursive::testing;
pub use self::cursive::{with_siv, SingletonView};
