//! Formatting and output helpers.
//!
//! We try to handle both textual output and interactive output (output to a
//! "TTY"). In the case of interactive output, we render with prettier non-ASCII
//! characters and with colors, using shell-specific escape codes.

use cursive::theme::{Effect, Style};
use cursive::utils::markup::StyledString;
use cursive::utils::span::Span;

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
    /// Whether or not ANSI escape codes should be emitted (e.g. to render
    /// color).
    pub should_write_ansi_escape_codes: bool,

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

    /// Arrow character used when printing a commit cycle.
    pub cycle_arrow: &'static str,

    /// Horizontal line character used when printing a commit cycle.
    pub cycle_horizontal_line: &'static str,

    /// Vertical line character used when printing a commit cycle.
    pub cycle_vertical_line: &'static str,

    /// Corner at the upper left of the arrow used when printing a commit cycle.
    pub cycle_upper_left_corner: &'static str,

    /// Corner at the lower left of the arrow used when printing a commit cycle.
    pub cycle_lower_left_corner: &'static str,
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
            should_write_ansi_escape_codes: false,
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
            cycle_arrow: ">",
            cycle_horizontal_line: "-",
            cycle_vertical_line: "|",
            cycle_upper_left_corner: ",",
            cycle_lower_left_corner: "`",
        }
    }

    /// Glyphs used for output to a TTY.
    pub fn pretty() -> Self {
        Glyphs {
            should_write_ansi_escape_codes: true,
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
            cycle_arrow: "ᐅ",
            cycle_horizontal_line: "─",
            cycle_vertical_line: "│",
            cycle_upper_left_corner: "┌",
            cycle_lower_left_corner: "└",
        }
    }
}

impl std::fmt::Debug for Glyphs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "<Glyphs pretty={:?}>",
            self.should_write_ansi_escape_codes
        )
    }
}

/// Helper to build `StyledString`s by combining multiple strings (both regular
/// `String`s and `StyledString`s).
pub struct StyledStringBuilder {
    elements: Vec<StyledString>,
}

impl Default for StyledStringBuilder {
    fn default() -> Self {
        StyledStringBuilder::new()
    }
}

impl StyledStringBuilder {
    /// Constructor.
    pub fn new() -> Self {
        Self {
            elements: Vec::new(),
        }
    }

    /// Append a plain-text string to the internal buffer.
    pub fn append_plain(mut self, text: impl AsRef<str>) -> Self {
        self.elements.push(StyledString::plain(text.as_ref()));
        self
    }

    /// Style the provided `text` using `style`, then append it to the internal
    /// buffer.
    pub fn append_styled(mut self, text: impl AsRef<str>, style: impl Into<Style>) -> Self {
        self.elements
            .push(StyledString::styled(text.as_ref(), style));
        self
    }

    /// Directly append the provided `StyledString` to the internal buffer.
    pub fn append(mut self, text: impl Into<StyledString>) -> Self {
        self.elements.push(text.into());
        self
    }

    /// Create a new `StyledString` using all the components in the internal
    /// buffer.
    pub fn build(self) -> StyledString {
        let mut result = StyledString::new();
        for element in self.elements {
            result.append(element);
        }
        result
    }

    /// Helper function to join a list of `StyledString`s into a single
    /// `StyledString`s, using the provided `delimiter`.
    pub fn join(delimiter: impl Into<String>, strings: Vec<StyledString>) -> StyledString {
        let mut result = Self::new();
        let mut is_first = true;
        let delimiter = delimiter.into();
        for string in strings {
            if is_first {
                is_first = false;
            } else {
                result = result.append_plain(delimiter.clone());
            }
            result = result.append(string);
        }
        result.into()
    }

    /// Helper function to turn a list of lines, each of which is a
    /// `StyledString`, into a single `StyledString` with a newline at the end
    /// of each line.
    pub fn from_lines(lines: Vec<StyledString>) -> StyledString {
        let mut result = Self::new();
        for line in lines {
            result = result.append(line);
            result = result.append_plain("\n");
        }
        result.into()
    }
}

/// Set the provided effect to all the internal spans of the styled string.
pub fn set_effect(mut string: StyledString, effect: Effect) -> StyledString {
    string.spans_raw_attr_mut().for_each(|span| {
        span.attr.effects.insert(effect);
    });
    string
}

impl From<StyledStringBuilder> for StyledString {
    fn from(builder: StyledStringBuilder) -> Self {
        builder.build()
    }
}

fn render_style_as_ansi(content: &str, style: Style) -> eyre::Result<String> {
    let Style { effects, color } = style;
    let output = {
        use console::style;
        use cursive::theme::{BaseColor, Color, ColorType};
        let output = content.to_string();
        match color.front {
            ColorType::Palette(_) => {
                eyre::bail!("Not implemented: using cursive palette colors")
            }
            ColorType::Color(Color::Rgb(..)) | ColorType::Color(Color::RgbLowRes(..)) => {
                eyre::bail!("Not implemented: using raw RGB colors")
            }
            ColorType::InheritParent | ColorType::Color(Color::TerminalDefault) => style(output),
            ColorType::Color(Color::Light(color)) => match color {
                BaseColor::Black => style(output).black().bright(),
                BaseColor::Red => style(output).red().bright(),
                BaseColor::Green => style(output).green().bright(),
                BaseColor::Yellow => style(output).yellow().bright(),
                BaseColor::Blue => style(output).blue().bright(),
                BaseColor::Magenta => style(output).magenta().bright(),
                BaseColor::Cyan => style(output).cyan().bright(),
                BaseColor::White => style(output).white().bright(),
            },
            ColorType::Color(Color::Dark(color)) => match color {
                BaseColor::Black => style(output).black(),
                BaseColor::Red => style(output).red(),
                BaseColor::Green => style(output).green(),
                BaseColor::Yellow => style(output).yellow(),
                BaseColor::Blue => style(output).blue(),
                BaseColor::Magenta => style(output).magenta(),
                BaseColor::Cyan => style(output).cyan(),
                BaseColor::White => style(output).white(),
            },
        }
    };

    let output = {
        let mut output = output;
        for effect in effects.iter() {
            output = match effect {
                Effect::Simple => output,
                Effect::Dim => output.dim(),
                Effect::Reverse => output.reverse(),
                Effect::Bold => output.bold(),
                Effect::Italic => output.italic(),
                Effect::Strikethrough => eyre::bail!("Not implemented: Effect::Strikethrough"),
                Effect::Underline => output.underlined(),
                Effect::Blink => output.blink(),
            };
        }
        output
    };

    Ok(output.to_string())
}

/// Write the provided string to `out`, using ANSI escape codfes as necessary to
/// style it.
///
/// TODO: return something that implements `Display` instead of a `String`.
pub fn printable_styled_string(glyphs: &Glyphs, string: StyledString) -> eyre::Result<String> {
    let result = string
        .spans()
        .map(|span| {
            let Span {
                content,
                attr,
                width: _,
            } = span;
            if glyphs.should_write_ansi_escape_codes {
                Ok(render_style_as_ansi(content, *attr)?)
            } else {
                Ok(content.to_string())
            }
        })
        .collect::<eyre::Result<String>>()?;
    Ok(result)
}
