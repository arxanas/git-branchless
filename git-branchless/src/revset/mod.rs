//! Parser and evaluator for a "revset"-like language, as in Mercurial and
//! Jujutsu.

mod ast;
mod builtins;
mod eval;
mod parser;
mod pattern;
mod resolve;

pub use ast::Expr;
pub use eval::eval;
pub use parser::parse;
pub use resolve::{check_revset_syntax, resolve_commits};

use lalrpop_util::lalrpop_mod;
lalrpop_mod!(
    #[allow(clippy::all, clippy::as_conversions)]
    grammar,
    "/revset/grammar.rs"
);
