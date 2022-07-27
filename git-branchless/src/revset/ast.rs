use std::borrow::Cow;
use std::fmt::Display;

/// A node in the parsed AST.
#[allow(missing_docs)]
#[derive(Clone, Debug)]
pub enum Expr<'input> {
    Name(Cow<'input, str>),
    FunctionCall(Cow<'input, str>, Vec<Expr<'input>>),
}

impl Display for Expr<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Expr::Name(name) => write!(f, "{}", name),
            Expr::FunctionCall(name, args) => {
                write!(f, "{}(", name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arg)?;
                }
                write!(f, ")")
            }
        }
    }
}
