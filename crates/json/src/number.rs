//! Numbers as JSON Schema compares them: exactly when they fit, as `f64` when they do not.
//!
//! A JSON number is kept as written by the reader. Here it becomes a decimal (`mantissa × 10^exponent`, 36 digits) so that `0.0075`
//! is a multiple of `0.0001` and `1.0` equals `1`; a number with more digits, or an exponent too large to align, is an `f64`.

use std::cmp::Ordering;

/// Mantissas have at most this many digits (an `i128` holds 38).
const MAX_DIGITS: usize = 36;
/// Aligning two decimals multiplies by a power of ten: no further than this.
const MAX_SHIFT: u32 = 36;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Num {
    /// `mantissa × 10^exponent`, with a mantissa that does not end in zero (zero is `0 × 10^0`).
    Decimal {
        mantissa: i128,
        exponent: i32,
    },
    Float(f64),
}

impl Num {
    /// A number as the reader wrote it (`-1.5e3`, and JSON5's `+1`, `.5`, `0x1F`, `Infinity`, `NaN`). One that has too many digits, or
    /// an exponent too far from zero to line up with another, is a float.
    pub(crate) fn parse(text: &str) -> Option<Num> {
        Self::exact(text).or_else(|| Self::float(text))
    }

    fn float(text: &str) -> Option<Num> {
        let (negative, body) = match text.as_bytes().first()? {
            b'-' => (true, text.get(1..)?),
            b'+' => (false, text.get(1..)?),
            _ => (false, text),
        };
        if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
            let digits: Option<Vec<u32>> = hex.chars().map(|c| c.to_digit(16)).collect();
            let digits = digits.filter(|d| !d.is_empty())?;
            let value = digits.iter().fold(0f64, |acc, d| acc * 16.0 + f64::from(*d));
            return Some(Num::Float(if negative { -value } else { value }));
        }
        text.parse::<f64>().ok().or_else(|| float_word(text)).map(Num::Float)
    }

    fn exact(text: &str) -> Option<Num> {
        let (negative, body) = match text.as_bytes().first()? {
            b'-' => (true, text.get(1..)?),
            b'+' => (false, text.get(1..)?),
            _ => (false, text),
        };
        if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
            let value = u128::from_str_radix(hex, 16).ok().and_then(|v| i128::try_from(v).ok())?;
            return Num::decimal(if negative { -value } else { value }, 0);
        }
        let (mantissa_text, exponent_text) = match body.find(['e', 'E']) {
            Some(at) => (body.get(..at)?, body.get(at + 1..)?),
            None => (body, "0"),
        };
        let (int, frac) = mantissa_text.split_once('.').unwrap_or((mantissa_text, ""));
        let digits: String = int.chars().chain(frac.chars()).collect();
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let exponent = exponent_text.parse::<i32>().ok()?;
        let trimmed = digits.trim_start_matches('0');
        if trimmed.len() > MAX_DIGITS {
            return None;
        }
        let value: i128 = if trimmed.is_empty() { 0 } else { trimmed.parse().ok()? };
        let scale = i32::try_from(frac.len()).ok()?;
        Num::decimal(if negative { -value } else { value }, exponent.checked_sub(scale)?)
    }

    /// The decimal `mantissa × 10^exponent`, normalised; none when the exponent does not fit once the trailing zeros are counted in.
    fn decimal(mut mantissa: i128, mut exponent: i32) -> Option<Num> {
        if mantissa == 0 {
            return Some(Num::Decimal { mantissa: 0, exponent: 0 });
        }
        while mantissa % 10 == 0 {
            mantissa /= 10;
            exponent = exponent.checked_add(1)?;
        }
        Some(Num::Decimal { mantissa, exponent })
    }

    pub(crate) fn to_f64(self) -> f64 {
        match self {
            Num::Float(f) => f,
            Num::Decimal { mantissa, exponent } => mantissa as f64 * 10f64.powi(exponent),
        }
    }

    pub(crate) fn is_integer(self) -> bool {
        match self {
            Num::Decimal { exponent, .. } => exponent >= 0,
            Num::Float(f) => f.is_finite() && f.fract() == 0.0,
        }
    }

    /// The numbers in order, or `None` when one is NaN.
    pub(crate) fn compare(self, other: Num) -> Option<Ordering> {
        if let (Num::Decimal { mantissa: a, exponent: ea }, Num::Decimal { mantissa: b, exponent: eb }) = (self, other)
            && let Some((x, y)) = align(a, ea, b, eb)
        {
            return Some(x.cmp(&y));
        }
        self.to_f64().partial_cmp(&other.to_f64())
    }

    /// Whether `self` is a whole multiple of `divisor` (which must be above zero; otherwise nothing is a multiple).
    pub(crate) fn is_multiple_of(self, divisor: Num) -> bool {
        if let (Num::Decimal { mantissa: a, exponent: ea }, Num::Decimal { mantissa: b, exponent: eb }) = (self, divisor)
            && b != 0
            && let Some((x, y)) = align(a, ea, b, eb)
        {
            return x % y == 0;
        }
        let (x, d) = (self.to_f64(), divisor.to_f64());
        if d <= 0.0 || !x.is_finite() || !d.is_finite() {
            return false;
        }
        let quotient = x / d;
        quotient.is_finite() && quotient.fract() == 0.0
    }

    pub(crate) fn sign(self) -> Ordering {
        self.compare(Num::Decimal { mantissa: 0, exponent: 0 }).unwrap_or(Ordering::Equal)
    }
}

/// `Infinity`, `-Infinity`, `+Infinity` and `NaN` (JSON5).
fn float_word(text: &str) -> Option<f64> {
    match text {
        "Infinity" | "+Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        "NaN" | "+NaN" | "-NaN" => Some(f64::NAN),
        _ => None,
    }
}

/// Both mantissas at the smaller exponent, when that fits.
fn align(a: i128, ea: i32, b: i128, eb: i32) -> Option<(i128, i128)> {
    let shift = |m: i128, by: i32| -> Option<i128> {
        let by = u32::try_from(by).ok().filter(|b| *b <= MAX_SHIFT)?;
        m.checked_mul(10i128.checked_pow(by)?)
    };
    match ea.cmp(&eb) {
        Ordering::Equal => Some((a, b)),
        Ordering::Greater => Some((shift(a, ea.checked_sub(eb)?)?, b)),
        Ordering::Less => Some((a, shift(b, eb.checked_sub(ea)?)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(text: &str) -> Num {
        Num::parse(text).unwrap_or_else(|| panic!("{text} is a number"))
    }

    #[test]
    fn numbers_compare_by_value_whatever_they_are_written_like() {
        assert!(n("1.0").compare(n("1")) == Some(Ordering::Equal));
        assert!(n("1e2").compare(n("100")) == Some(Ordering::Equal));
        assert!(n("0.10").compare(n("0.1")) == Some(Ordering::Equal));
        assert!(n("-0").compare(n("0")) == Some(Ordering::Equal));
        assert_eq!(n("0.1").compare(n("0.10000000000000001")), Some(Ordering::Less), "exact, where a float would say equal");
        assert_eq!(n("1e400").compare(n("1e399")), Some(Ordering::Greater));
        assert_eq!(n("12345678901234567890123456789012345678901234567890").compare(n("1")), Some(Ordering::Greater));
        assert_eq!(n("NaN").compare(n("1")), None);
        assert_eq!(n("0x10").compare(n("16")), Some(Ordering::Equal));
    }

    #[test]
    fn integers_are_numbers_without_a_fraction() {
        for yes in ["1", "1.0", "1e2", "-5", "0", "1E+3", "0.0"] {
            assert!(n(yes).is_integer(), "{yes}");
        }
        for no in ["1.5", "1e-1", "0.001"] {
            assert!(!n(no).is_integer(), "{no}");
        }
    }

    #[test]
    fn multiples_are_exact_in_decimal() {
        assert!(n("0.0075").is_multiple_of(n("0.0001")));
        assert!(n("4.5").is_multiple_of(n("1.5")));
        assert!(!n("35").is_multiple_of(n("1.5")));
        assert!(n("12391239123").is_multiple_of(n("1")));
        assert!(!n("1e308").is_multiple_of(n("0.123456789")), "a quotient out of range is no multiple");
        assert!(!n("1").is_multiple_of(n("0")));
        assert!(n("0").is_multiple_of(n("7")));
    }

    #[test]
    fn numbers_that_do_not_line_up_exactly_are_floats_not_nothing() {
        for text in [
            "0x1FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
            "-0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
            "1e2147483647",
            "10e2147483647",
            "0.5e-2147483648",
            "1.5e-2147483647",
        ] {
            assert!(Num::parse(text).is_some(), "{text}");
        }
        assert!(matches!(Num::parse("0x1FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF"), Some(Num::Float(f)) if f > 1e48));
        assert!(
            matches!(Num::parse("10e2147483647"), Some(Num::Float(_))),
            "the trailing zero would push the exponent out of range: not an exact number"
        );
        assert!(Num::parse("0x").is_none() && Num::parse("0xZ").is_none());
    }

    #[test]
    fn nothing_here_panics_on_odd_text() {
        for text in
            ["", "-", "+", ".", "e", "1e", "e1", "1e99999999999", "--1", "0x", "0xZZ", "1.2.3", "٣", "1e-99999999999", "9".repeat(80).as_str()]
        {
            let _ = Num::parse(text);
        }
        assert!(Num::parse("1e99999999999").is_some_and(|x| x.to_f64().is_infinite()));
    }
}
