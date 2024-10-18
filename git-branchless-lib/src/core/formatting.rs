//! Formatting and output helpers.
//!
//! We try to handle both textual output and interactive output (output to a
//! "TTY"). In the case of interactive output, we render with prettier non-ASCII
//! characters and with colors, using shell-specific escape codes.

use std::fmt::Display;

use cursive::theme::{Effect, Style};
use cursive::utils::markup::StyledString;
use cursive::utils::span::Span;

/// Pluralize a quantity, as appropriate. Example:
///
/// ```
/// # use branchless::core::formatting::Pluralize;
/// let p = Pluralize {
///     determiner: None,
///     amount: 1,
///     unit: ("thing", "things"),
/// };
/// assert_eq!(p.to_string(), "1 thing");
///
/// let p = Pluralize {
///     determiner: Some(("this", "these")),
///     amount: 2,
///     unit: ("thing", "things")
/// };
/// assert_eq!(p.to_string(), "these 2 things");
/// ```
pub struct Pluralize<'a> {
    /// The string to render before the amount if the amount is singular vs plural.
    pub determiner: Option<(&'a str, &'a str)>,

    /// The amount of the quantity.
    pub amount: usize,

    /// The string to render after the amount if the amount is singular vs plural.
    pub unit: (&'a str, &'a str),
}

impl Display for Pluralize<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self {
                amount: 1,
                unit: (unit, _),
                determiner: None,
            } => write!(f, "{} {}", 1, unit),

            Self {
                amount,
                unit: (_, unit),
                determiner: None,
            } => write!(f, "{amount} {unit}"),

            Self {
                amount: 1,
                unit: (unit, _),
                determiner: Some((determiner, _)),
            } => write!(f, "{} {} {}", determiner, 1, unit),

            Self {
                amount,
                unit: (_, unit),
                determiner: Some((_, determiner)),
            } => write!(f, "{determiner} {amount} {unit}"),
        }
    }
}

/// Glyphs to use for rendering the smartlog.
#[derive(Clone)]
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
    pub split: &'static str,

    /// Line used to connect a child commit to its non-first parent commit.
    pub merge: &'static str,

    /// Cursor for a normal visible commit which is not currently checked out.
    pub commit_visible: &'static str,

    /// Cursor for the visible commit which is currently checked out.
    pub commit_visible_head: &'static str,

    /// Cursor for an obsolete commit.
    pub commit_obsolete: &'static str,

    /// Cursor for the obsolete commit which is currently checked out.
    pub commit_obsolete_head: &'static str,

    /// Cursor for a commit belonging to the main branch, which is not currently
    /// checked out.
    pub commit_main: &'static str,

    /// Cursor for a commit belonging to the main branch, which is currently
    /// checked out.
    pub commit_main_head: &'static str,

    /// Cursor for an obsolete commit belonging to the main branch. (This is an
    /// unusual situation.)
    pub commit_main_obsolete: &'static str,

    /// Cursor for an obsolete commit belonging to the main branch, which is
    /// currently checked out. (This is an unusual situation.)
    pub commit_main_obsolete_head: &'static str,

    /// Cursor indicating that some number of commits have been omitted from the
    /// smartlog at this position.
    pub commit_omitted: &'static str,

    /// Cursor indicating that a commit was either merging into this child
    /// commit or merged from this parent commit.
    pub commit_merge: &'static str,

    /// Character used to point to the currently-checked-out branch.
    pub branch_arrow: &'static str,

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
        let color_support = concolor::get(concolor::Stream::Stdout);
        if color_support.color() {
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
            split: "\\",
            merge: "/",
            commit_visible: "o",
            commit_visible_head: "@",
            commit_obsolete: "x",
            commit_obsolete_head: "%",
            commit_main: "O",
            commit_main_head: "@",
            commit_main_obsolete: "X",
            commit_main_obsolete_head: "%",
            commit_omitted: "#",
            commit_merge: "&",
            branch_arrow: ">",
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
            split: "━┓",
            merge: "━┛",
            commit_visible: "○",
            commit_visible_head: "●",
            commit_obsolete: "✕",
            commit_obsolete_head: "⦻",
            commit_omitted: "◌",
            commit_merge: "↓",
            commit_main: "◇",
            commit_main_head: "◆",
            commit_main_obsolete: "✕",
            commit_main_obsolete_head: "❖",
            branch_arrow: "ᐅ",
            bullet_point: "•",
            cycle_arrow: "ᐅ",
            cycle_horizontal_line: "─",
            cycle_vertical_line: "│",
            cycle_upper_left_corner: "┌",
            cycle_lower_left_corner: "└",
        }
    }

    /// Return a `Glyphs` object suitable for rendering graphs in the reverse of
    /// their usual order.
    pub fn reverse_order(mut self, reverse: bool) -> Self {
        if reverse {
            std::mem::swap(&mut self.split, &mut self.merge);
        }
        self
    }

    /// Write the provided string to `out`, using ANSI escape codes as necessary to
    /// style it.
    ///
    /// TODO: return something that implements `Display` instead of a `String`.
    pub fn render(&self, string: StyledString) -> eyre::Result<String> {
        let result = string
            .spans()
            .map(|span| {
                let Span {
                    content,
                    attr,
                    width: _,
                } = span;
                if self.should_write_ansi_escape_codes {
                    Ok(render_style_as_ansi(content, *attr)?)
                } else {
                    Ok(content.to_string())
                }
            })
            .collect::<eyre::Result<String>>()?;
        Ok(result)
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

    fn append_plain_inner(mut self, text: &str) -> Self {
        self.elements.push(StyledString::plain(text));
        self
    }

    /// Append a plain-text string to the internal buffer.
    pub fn append_plain(self, text: impl AsRef<str>) -> Self {
        self.append_plain_inner(text.as_ref())
    }

    fn append_styled_inner(mut self, text: &str, style: Style) -> Self {
        self.elements.push(StyledString::styled(text, style));
        self
    }

    /// Style the provided `text` using `style`, then append it to the internal
    /// buffer.
    pub fn append_styled(self, text: impl AsRef<str>, style: impl Into<Style>) -> Self {
        self.append_styled_inner(text.as_ref(), style.into())
    }

    fn append_inner(mut self, text: StyledString) -> Self {
        self.elements.push(text);
        self
    }

    /// Directly append the provided `StyledString` to the internal buffer.
    pub fn append(self, text: impl Into<StyledString>) -> Self {
        self.append_inner(text.into())
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
    pub fn join(delimiter: &str, strings: Vec<StyledString>) -> StyledString {
        let mut result = Self::new();
        let mut is_first = true;
        for string in strings {
            if is_first {
                is_first = false;
            } else {
                result = result.append_plain(delimiter);
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

    // `StyledObject` will try to do its own detection of whether or not it
    // should render ANSI escape codes. Disable that detection and use whatever
    // we've determined, so that the user can force color on or off. (The caller
    // will only call this function if the user wants color, so we pass `true`.)
    // See https://github.com/arxanas/git-branchless/issues/506
    let output = output.force_styling(true);

    Ok(output.to_string())
}
