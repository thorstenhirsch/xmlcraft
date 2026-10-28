//! Decimal numbers of any size: the value space of `decimal` and the types derived from it (`integer`, `long`, …).

use std::cmp::Ordering;
use std::fmt;

/// A decimal number: sign, digits before the point (no leading zeros) and after it (no trailing zeros). Zero is never negative.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Decimal {
    negative: bool,
    int: String,
    frac: String,
}

impl Decimal {
    pub fn zero() -> Self {
        Self { negative: false, int: "0".to_owned(), frac: String::new() }
    }

    /// The lexical form of `decimal`: an optional sign, digits with an optional point, at least one digit (`12`, `-1.5`, `.5`, `5.`).
    pub fn parse(lexical: &str) -> Option<Self> {
        let (negative, rest) = split_sign(lexical);
        let (int, frac) = match rest.split_once('.') {
            Some((int, frac)) => (int, frac),
            None => (rest, ""),
        };
        if !all_digits(int) || !all_digits(frac) || (int.is_empty() && frac.is_empty()) {
            return None;
        }
        Some(Self::from_parts(negative, int, frac))
    }

    /// The lexical form of `integer`: an optional sign and digits, no point.
    pub fn parse_integer(lexical: &str) -> Option<Self> {
        let (negative, rest) = split_sign(lexical);
        if rest.is_empty() || !all_digits(rest) {
            return None;
        }
        Some(Self::from_parts(negative, rest, ""))
    }

    fn from_parts(negative: bool, int: &str, frac: &str) -> Self {
        let int = int.trim_start_matches('0');
        let frac = frac.trim_end_matches('0');
        let is_zero = int.is_empty() && frac.is_empty();
        Self { negative: negative && !is_zero, int: if int.is_empty() { "0".to_owned() } else { int.to_owned() }, frac: frac.to_owned() }
    }

    /// A whole number.
    pub fn from_i64(value: i64) -> Self {
        Self::from_parts(value < 0, &value.unsigned_abs().to_string(), "")
    }

    pub fn is_negative(&self) -> bool {
        self.negative
    }

    pub fn is_zero(&self) -> bool {
        self.int == "0" && self.frac.is_empty()
    }

    /// Whether the number has no fractional part.
    pub fn is_integer(&self) -> bool {
        self.frac.is_empty()
    }

    /// The digits after the point, without trailing zeros: the number `fractionDigits` constrains.
    pub fn fraction_digits(&self) -> usize {
        self.frac.len()
    }

    /// The digits of the value, the number `totalDigits` constrains: those before the point (none for a number below one) and after
    /// it; zero has one.
    pub fn total_digits(&self) -> usize {
        let int = if self.int == "0" { 0 } else { self.int.len() };
        (int + self.frac.len()).max(1)
    }

    /// The value as a whole number, when it is one and fits.
    pub fn to_i128(&self) -> Option<i128> {
        if !self.frac.is_empty() {
            return None;
        }
        let magnitude: i128 = self.int.parse().ok()?;
        Some(if self.negative { -magnitude } else { magnitude })
    }

    /// The digits before the point.
    pub fn integer_digits(&self) -> &str {
        &self.int
    }

    /// The digits after the point.
    pub fn fraction_part(&self) -> &str {
        &self.frac
    }

    /// `-self`.
    pub fn negated(&self) -> Self {
        Self { negative: !self.negative && !self.is_zero(), ..self.clone() }
    }

    fn magnitude_cmp(&self, other: &Self) -> Ordering {
        self.int.len().cmp(&other.int.len()).then_with(|| self.int.cmp(&other.int)).then_with(|| {
            // Digit by digit, a missing digit being 0 (the fractions have no trailing zeros, so this settles it).
            self.frac.cmp(&other.frac)
        })
    }
}

fn split_sign(text: &str) -> (bool, &str) {
    match text.as_bytes().first() {
        Some(b'-') => (true, text.get(1..).unwrap_or_default()),
        Some(b'+') => (false, text.get(1..).unwrap_or_default()),
        _ => (false, text),
    }
}

fn all_digits(text: &str) -> bool {
    text.bytes().all(|b| b.is_ascii_digit())
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => self.magnitude_cmp(other),
            (true, true) => other.magnitude_cmp(self),
        }
    }
}

/// The canonical representation: no plus sign, no leading or trailing zeros, a point only when there are fractional digits (`-1.5`, `0`).
impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        f.write_str(&self.int)?;
        if !self.frac.is_empty() {
            write!(f, ".{}", self.frac)?;
        }
        Ok(())
    }
}
