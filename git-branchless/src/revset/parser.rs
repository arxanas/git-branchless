use lazy_static::lazy_static;
use regex::Regex;
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
/// To update the grammar, modify `grammar.lalrpop`.
#[instrument]
pub fn parse(s: &str) -> Result<Expr, ParseError> {
    ExprParser::new().parse(s).map_err(|err| {
        let message = err.to_string();

        // HACK: `lalrpop` doesn't let us customize the text of the string
        // literal token, so replace it after the fact.
        lazy_static! {
            static ref OBJECT_RE: Regex = Regex::new("r#\"\\[[^\"]+\"#").unwrap();
            static ref STRING_LITERAL_RE: Regex = Regex::new("r#\"\\\\[^\"]+\"#").unwrap();
        }
        let message = OBJECT_RE.replace(&message, "a commit/branch/tag");
        let message = STRING_LITERAL_RE.replace(&message, "a string literal");

        ParseError::ParseError(message.into_owned())
    })
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
                "Unrecognized EOF found at 5\nExpected one of \"!\", \"(\", \"::\", \"not \", a commit/branch/tag or a string literal",
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
