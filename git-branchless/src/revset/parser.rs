use thiserror::Error;
use tracing::instrument;

use super::grammar::ExprParser;
use super::Expr;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("parse error: {0}")]
    ParseError(String),
}

/// Parse a string representing a revset expression into an [Expr].
///
/// To update the grammar, make a change to `grammar.lalrpop`, then run
///
/// ```text
///  lalrpop path/to/grammar.lalrpop
/// ```
///
/// to regenerate the source file.
#[instrument]
pub fn parse(s: &str) -> Result<Expr, ParseError> {
    ExprParser::new()
        .parse(s)
        .map_err(|err| ParseError::ParseError(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_revset_parser() -> eyre::Result<()> {
        insta::assert_debug_snapshot!(parse("foo | bar")?, @r###"
        Fn(
            "union",
            [
                Name(
                    "foo",
                ),
                Name(
                    "bar",
                ),
            ],
        )
        "###);

        insta::assert_debug_snapshot!(parse("foo |"), @r###"
        Err(
            ParseError(
                "Unrecognized EOF found at 5\nExpected one of \"(\", \"::\", r#\"[a-zA-Z0-9/_$@.-]+\"# or r#\"\\\\x22([^\\\\x22\\\\x5c]|\\\\x5c.)*\\\\x22\"#",
            ),
        )
        "###);

        insta::assert_debug_snapshot!(parse("foo-bar/baz:qux-grault"), @r###"
        Ok(
            Fn(
                "range",
                [
                    Name(
                        "foo-bar/baz",
                    ),
                    Name(
                        "qux-grault",
                    ),
                ],
            ),
        )
        "###);

        Ok(())
    }
}
