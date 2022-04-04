//! Utilities to control output and render to the terminal.

mod cursive;
mod prompt;

pub use self::cursive::testing;
pub use self::cursive::{with_siv, SingletonView};
pub use prompt::prompt_select_commit;
