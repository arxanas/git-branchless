//! Formatting and output helpers.
//!
//! We try to handle both textual output and interactive output (output to a
//! "TTY"). In the case of interactive output, we render with prettier non-ASCII
//! characters and with colors, using shell-specific escape codes.

/// Pluralize a quantity, as appropriate. Example:
///
/// ```
/// # use branchless::formatting::Pluralize;
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
