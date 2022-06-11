//! Parser and evaluator for a "revset"-like language, as in Mercurial and
//! Jujutsu.

mod ast;
mod eval;
mod parser;
mod resolve;

#[rustfmt::skip]
#[allow(clippy::all, clippy::as_conversions)]
mod grammar;

pub use ast::Expr;
pub use eval::eval;
pub use parser::parse;
pub use resolve::resolve_commits;
