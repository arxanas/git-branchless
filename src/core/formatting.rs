//! Formatting and output helpers.
//!
//! We try to handle both textual output and interactive output (output to a
//! "TTY"). In the case of interactive output, we render with prettier non-ASCII
//! characters and with colors, using shell-specific escape codes.

/// Pluralize a quantity, as appropriate. Example:
///
/// ```
/// # use branchless::core::formatting::Pluralize;
/// let p = Pluralize { amount: 1, singular: "thing", plural: "things"};
/// assert_eq!(p.to_string(), "1 thing");
///
/// let p = Pluralize { amount: 2, singular: "thing", plural: "things"};
/// assert_eq!(p.to_string(), "2 things");
/// ```
pub struct Pluralize<'a> {
    /// The amount of the quantity.
    pub amount: isize,

    /// The string to render if the amount is singular.
    pub singular: &'a str,

    /// The string to render if the amount is plural.uee
    pub plural: &'a str,
}

impl<'a> ToString for Pluralize<'a> {
    fn to_string(&self) -> String {
        match self.amount {
            1 => format!("{} {}", self.amount, self.singular),
            _ => format!("{} {}", self.amount, self.plural),
        }
    }
}

/// Glyphs to use for rendering the smartlog.
pub struct Glyphs {
    /// Line connecting a parent commit to its single child commit.
    pub line: &'static str,

    /// Line connecting a parent commit with two or more child commits.
    pub line_with_offshoot: &'static str,

    /// Denotes an omitted sequence of commits.
    pub vertical_ellipsis: &'static str,

    /// Line used to connect a parent commit to its non-first child commit.
    pub slash: &'static str,

    /// Cursor for a visible commit which is not currently checked out.
    pub commit_visible: &'static str,

    /// Cursor for the visible commit which is currently checked out.
    pub commit_visible_head: &'static str,

    /// Cursor for a hidden commit.
    pub commit_hidden: &'static str,

    /// Cursor for the hidden commit which is currently checked out.
    pub commit_hidden_head: &'static str,

    /// Cursor for a commit belonging to the main branch, which is not currently
    /// checked out.
    pub commit_main: &'static str,

    /// Cursor for a commit belonging to the main branch, which is currently
    /// checked out.
    pub commit_main_head: &'static str,

    /// Cursor for a hidden commit belonging to the main branch. (This is an
    /// unusual situation.)
    pub commit_main_hidden: &'static str,

    /// Cursor for a hidden commit belonging to the main branch, which is
    /// currently checked out. (This is an unusual situation.)
    pub commit_main_hidden_head: &'static str,

    /// Bullet-point character for a list of newline-separated items.
    pub bullet_point: &'static str,
}

impl Glyphs {
    /// Make the `Glyphs` object appropriate for `stdout`.
    pub fn detect() -> Self {
        if console::user_attended() {
            Glyphs::pretty()
        } else {
            Glyphs::text()
        }
    }

    /// Glyphs used for output to a text file or non-TTY.
    pub fn text() -> Self {
        Glyphs {
            line: "|",
            line_with_offshoot: "|",
            vertical_ellipsis: ":",
            slash: "\\",
            commit_visible: "o",
            commit_visible_head: "@",
            commit_hidden: "x",
            commit_hidden_head: "%",
            commit_main: "O",
            commit_main_head: "@",
            commit_main_hidden: "X",
            commit_main_hidden_head: "%",
            bullet_point: "-",
        }
    }

    /// Glyphs used for output to a TTY.
    fn pretty() -> Self {
        Glyphs {
            line: "┃",
            line_with_offshoot: "┣",
            vertical_ellipsis: "⋮",
            slash: "━┓",
            commit_visible: "◯",
            commit_visible_head: "●",
            commit_hidden: "✕",
            commit_hidden_head: "⦻",
            commit_main: "◇",
            commit_main_head: "◆",
            commit_main_hidden: "✕",
            commit_main_hidden_head: "❖",
            bullet_point: "•",
        }
    }
}
