//! Parser and evaluator for a "revset"-like language, as in Mercurial and
//! Jujutsu.

#![warn(missing_docs)]
#![warn(
    clippy::all,
    clippy::as_conversions,
    clippy::clone_on_ref_ptr,
    clippy::dbg_macro
)]
#![allow(clippy::too_many_arguments, clippy::blocks_in_conditions)]

mod ast;
mod builtins;
mod eval;
mod parser;
mod pattern;
mod resolve;

pub use ast::Expr;
pub use eval::eval;
pub use parser::parse;
pub use resolve::{check_revset_syntax, resolve_commits, resolve_default_smartlog_commits};

use lalrpop_util::lalrpop_mod;
lalrpop_mod!(
    #[allow(clippy::all, clippy::as_conversions, dead_code)]
    grammar,
    "/grammar.rs"
);
