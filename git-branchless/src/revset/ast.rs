use std::borrow::Cow;
use std::collections::HashMap;
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

impl<'input> Expr<'input> {
    /// Replace names in this expression with arbitrary expressions.
    ///
    /// Given a HashMap of names to Expr's, build a new Expr by crawling this
    /// one and replacing any names contained in the map with the corresponding
    /// Expr.
    pub fn replace_names(&self, map: &HashMap<String, Expr<'input>>) -> Expr<'input> {
        match self {
            Expr::Name(name) => match map.get(&name.to_string()) {
                Some(expr) => expr.clone(),
                None => self.clone(),
            },
            Expr::FunctionCall(name, args) => {
                let args = args.iter().map(|arg| arg.replace_names(map)).collect();
                Expr::FunctionCall(name.clone(), args)
            }
        }
    }
}
