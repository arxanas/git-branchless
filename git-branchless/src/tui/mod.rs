//! Utilities to control output and render to the terminal.

mod cursive;
mod prompt;

pub use self::cursive::{with_siv, SingletonView};
pub use git_record::testing;
pub use prompt::prompt_select_commit;
