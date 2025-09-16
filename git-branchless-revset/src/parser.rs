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
pub fn parse(s: &str) -> Result<Expr<'_>, ParseError> {
    ExprParser::new().parse(s).map_err(|err| {
        let message = err.to_string();

        // HACK: `lalrpop` doesn't let us customize the text of the string
        // literal token, so replace it after the fact.
        lazy_static! {
            // NOTE: the `lalrpop` output contains Rust raw string literals, so
            // we need to match those as well. However, the `#` character is
            // interpreted by insignificant-whitespace mode as a comment, so we
            // use `\x23` instead.
            static ref OBJECT_RE: Regex = Regex::new(
                r#"(?x)
                    r\x23"
                    \(
                    \[
                    [^"]+
                    "\x23
                "#
            )
            .unwrap();
            static ref STRING_LITERAL_RE: Regex = Regex::new(
                r#"(?x)
                    r\x23"
                    \\
                    [^"]+
                    "\x23
                "#
            )
            .unwrap();
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
        insta::assert_debug_snapshot!(parse("hello"), @r###"
        Ok(
            Name(
                "hello",
            ),
        )
        "###);
        Ok(())
    }

    #[test]
    fn test_revset_parse_function_calls() -> eyre::Result<()> {
        insta::assert_debug_snapshot!(parse("foo()"), @r###"
        Ok(
            FunctionCall(
                "foo",
                [],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo(bar)"), @r###"
        Ok(
            FunctionCall(
                "foo",
                [
                    Name(
                        "bar",
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo(bar, baz)"), @r###"
        Ok(
            FunctionCall(
                "foo",
                [
                    Name(
                        "bar",
                    ),
                    Name(
                        "baz",
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo(bar, baz,)"), @r###"
        Ok(
            FunctionCall(
                "foo",
                [
                    Name(
                        "bar",
                    ),
                    Name(
                        "baz",
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo(,)"), @r###"
        Err(
            ParseError(
                "Unrecognized token `,` found at 4:5\nExpected one of \"(\", \")\", \"..\", \":\", \"::\", a commit/branch/tag or a string literal",
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo(,bar)"), @r###"
        Err(
            ParseError(
                "Unrecognized token `,` found at 4:5\nExpected one of \"(\", \")\", \"..\", \":\", \"::\", a commit/branch/tag or a string literal",
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo(bar,,)"), @r###"
        Err(
            ParseError(
                "Unrecognized token `,` found at 8:9\nExpected one of \"(\", \")\", \"..\", \":\", \"::\", a commit/branch/tag or a string literal",
            ),
        )
        "###);
        Ok(())
    }

    #[test]
    fn test_revset_parse_set_operators() -> eyre::Result<()> {
        insta::assert_debug_snapshot!(parse("foo | bar & bar"), @r###"
        Ok(
            FunctionCall(
                "union",
                [
                    Name(
                        "foo",
                    ),
                    FunctionCall(
                        "intersection",
                        [
                            Name(
                                "bar",
                            ),
                            Name(
                                "bar",
                            ),
                        ],
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo & bar | bar")?, @r###"
        FunctionCall(
            "union",
            [
                FunctionCall(
                    "intersection",
                    [
                        Name(
                            "foo",
                        ),
                        Name(
                            "bar",
                        ),
                    ],
                ),
                Name(
                    "bar",
                ),
            ],
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo | bar")?, @r###"
        FunctionCall(
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
        insta::assert_debug_snapshot!(parse("foo | bar - baz")?, @r###"
        FunctionCall(
            "union",
            [
                Name(
                    "foo",
                ),
                FunctionCall(
                    "difference",
                    [
                        Name(
                            "bar",
                        ),
                        Name(
                            "baz",
                        ),
                    ],
                ),
            ],
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo |"), @r###"
        Err(
            ParseError(
                "Unrecognized EOF found at 5\nExpected one of \"(\", \"..\", \":\", \"::\", a commit/branch/tag or a string literal",
            ),
        )
        "###);
        Ok(())
    }

    #[test]
    fn test_revset_parse_range_operator() -> eyre::Result<()> {
        insta::assert_debug_snapshot!(parse("foo:bar"), @r###"
        Ok(
            FunctionCall(
                "range",
                [
                    Name(
                        "foo",
                    ),
                    Name(
                        "bar",
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo:"), @r###"
        Ok(
            FunctionCall(
                "descendants",
                [
                    Name(
                        "foo",
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse(":foo"), @r###"
        Ok(
            FunctionCall(
                "ancestors",
                [
                    Name(
                        "foo",
                    ),
                ],
            ),
        )
        "###);

        insta::assert_debug_snapshot!(parse("foo-bar/baz:qux-grault"), @r###"
        Ok(
            FunctionCall(
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

        insta::assert_debug_snapshot!(parse("foo..bar"), @r###"
        Ok(
            FunctionCall(
                "only",
                [
                    Name(
                        "bar",
                    ),
                    Name(
                        "foo",
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo.."), @r###"
        Ok(
            FunctionCall(
                "only",
                [
                    Name(
                        ".",
                    ),
                    Name(
                        "foo",
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("..bar"), @r###"
        Ok(
            FunctionCall(
                "only",
                [
                    Name(
                        "bar",
                    ),
                    Name(
                        ".",
                    ),
                ],
            ),
        )
        "###);

        Ok(())
    }

    #[test]
    fn test_revset_parse_string() -> eyre::Result<()> {
        insta::assert_debug_snapshot!(parse(r#" "" "#), @r###"
        Ok(
            Name(
                "",
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse(r#" "foo" "#), @r###"
        Ok(
            Name(
                "foo",
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse(r#" "foo bar" "#), @r###"
        Ok(
            Name(
                "foo bar",
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse(r#" "foo\nbar\\baz" "#), @r###"
        Ok(
            Name(
                "foo\nba\r\\\\baz",
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse(r" 'foo\nbar\\baz' "), @r###"
        Ok(
            Name(
                "foo\nba\r\\\\baz",
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse(r#" foo('bar') - baz(qux('qubit')) "#), @r###"
        Ok(
            FunctionCall(
                "difference",
                [
                    FunctionCall(
                        "foo",
                        [
                            Name(
                                "bar",
                            ),
                        ],
                    ),
                    FunctionCall(
                        "baz",
                        [
                            FunctionCall(
                                "qux",
                                [
                                    Name(
                                        "qubit",
                                    ),
                                ],
                            ),
                        ],
                    ),
                ],
            ),
        )
        "###);

        Ok(())
    }

    #[test]
    fn test_revset_parse_parentheses() -> eyre::Result<()> {
        insta::assert_debug_snapshot!(parse("((foo()))"), @r###"
        Ok(
            FunctionCall(
                "foo",
                [],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("(foo) - bar"), @r###"
        Ok(
            FunctionCall(
                "difference",
                [
                    Name(
                        "foo",
                    ),
                    Name(
                        "bar",
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo - (bar)"), @r###"
        Ok(
            FunctionCall(
                "difference",
                [
                    Name(
                        "foo",
                    ),
                    Name(
                        "bar",
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("(foo) & bar"), @r###"
        Ok(
            FunctionCall(
                "intersection",
                [
                    Name(
                        "foo",
                    ),
                    Name(
                        "bar",
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo & (bar)"), @r###"
        Ok(
            FunctionCall(
                "intersection",
                [
                    Name(
                        "foo",
                    ),
                    Name(
                        "bar",
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("(foo | bar):"), @r###"
        Ok(
            FunctionCall(
                "descendants",
                [
                    FunctionCall(
                        "union",
                        [
                            Name(
                                "foo",
                            ),
                            Name(
                                "bar",
                            ),
                        ],
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("(foo)^"), @r###"
        Ok(
            FunctionCall(
                "parents.nth",
                [
                    Name(
                        "foo",
                    ),
                    Name(
                        "1",
                    ),
                ],
            ),
        )
        "###);

        Ok(())
    }

    #[test]
    fn test_revset_parse_git_revision_syntax() -> eyre::Result<()> {
        insta::assert_debug_snapshot!(parse("foo:bar^"), @r###"
        Ok(
            FunctionCall(
                "range",
                [
                    Name(
                        "foo",
                    ),
                    FunctionCall(
                        "parents.nth",
                        [
                            Name(
                                "bar",
                            ),
                            Name(
                                "1",
                            ),
                        ],
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo|bar^"), @r###"
        Ok(
            FunctionCall(
                "union",
                [
                    Name(
                        "foo",
                    ),
                    FunctionCall(
                        "parents.nth",
                        [
                            Name(
                                "bar",
                            ),
                            Name(
                                "1",
                            ),
                        ],
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo:bar^3"), @r###"
        Ok(
            FunctionCall(
                "range",
                [
                    Name(
                        "foo",
                    ),
                    FunctionCall(
                        "parents.nth",
                        [
                            Name(
                                "bar",
                            ),
                            Name(
                                "3",
                            ),
                        ],
                    ),
                ],
            ),
        )
        "###);

        insta::assert_debug_snapshot!(parse("foo:bar~"), @r###"
        Ok(
            FunctionCall(
                "range",
                [
                    Name(
                        "foo",
                    ),
                    FunctionCall(
                        "ancestors.nth",
                        [
                            Name(
                                "bar",
                            ),
                            Name(
                                "1",
                            ),
                        ],
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo|bar~"), @r###"
        Ok(
            FunctionCall(
                "union",
                [
                    Name(
                        "foo",
                    ),
                    FunctionCall(
                        "ancestors.nth",
                        [
                            Name(
                                "bar",
                            ),
                            Name(
                                "1",
                            ),
                        ],
                    ),
                ],
            ),
        )
        "###);
        insta::assert_debug_snapshot!(parse("foo:bar~3"), @r###"
        Ok(
            FunctionCall(
                "range",
                [
                    Name(
                        "foo",
                    ),
                    FunctionCall(
                        "ancestors.nth",
                        [
                            Name(
                                "bar",
                            ),
                            Name(
                                "3",
                            ),
                        ],
                    ),
                ],
            ),
        )
        "###);

        Ok(())
    }
}
