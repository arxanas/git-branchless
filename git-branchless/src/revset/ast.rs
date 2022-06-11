use std::borrow::Cow;

/// A node in the parsed AST.
#[derive(Clone, Debug)]
pub enum Expr {
    /// A plain string name.
    Name(String),

    /// A function call.
    Fn(Cow<'static, str>, Vec<Expr>),
}
