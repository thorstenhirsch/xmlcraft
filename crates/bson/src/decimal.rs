//! Decimal128 (IEEE 754-2008, the BID encoding BSON stores) to and from its string form, as the BSON specification's Extended JSON
//! uses it.

const BIAS: i32 = 6176;
const MAX_EXPONENT: i32 = 6111;
const MIN_EXPONENT: i32 = -6176;
const MAX_DIGITS: usize = 34;

/// The 16 stored bytes as text: `1.5`, `-0.001`, `1E+3`, `Infinity`, `NaN`.
pub fn to_string(bytes: &[u8; 16]) -> String {
    let v = u128::from_le_bytes(*bytes);
    let negative = v >> 127 == 1;
    let sign = if negative { "-" } else { "" };
    let top = (v >> 122) & 0x1F;
    if top == 0b11111 {
        return "NaN".to_owned();
    }
    if top == 0b11110 {
        return format!("{sign}Infinity");
    }
    let (coefficient, exponent) = if (v >> 125) & 0b11 == 0b11 {
        // The large form has a coefficient above 10^34: not a canonical value, read as zero.
        (0u128, i32::try_from((v >> 111) & 0x3FFF).unwrap_or(0) - BIAS)
    } else {
        (v & ((1u128 << 113) - 1), i32::try_from((v >> 113) & 0x3FFF).unwrap_or(0) - BIAS)
    };
    let coefficient = if coefficient > 9_999_999_999_999_999_999_999_999_999_999_999 { 0 } else { coefficient };
    let digits = coefficient.to_string();
    let adjusted = exponent + i32::try_from(digits.len()).unwrap_or(0) - 1;
    // The to-scientific-string rule of the General Decimal Arithmetic specification.
    let body = if exponent <= 0 && adjusted >= -6 {
        if exponent == 0 {
            digits
        } else {
            let places = usize::try_from(-exponent).unwrap_or(0);
            if digits.len() > places {
                let (int, frac) = digits.split_at(digits.len() - places);
                format!("{int}.{frac}")
            } else {
                format!("0.{}{digits}", "0".repeat(places - digits.len()))
            }
        }
    } else {
        let (first, rest) = digits.split_at(1);
        let mantissa = if rest.is_empty() { first.to_owned() } else { format!("{first}.{rest}") };
        format!("{mantissa}E{}{}", if adjusted >= 0 { "+" } else { "-" }, adjusted.abs())
    };
    format!("{sign}{body}")
}

/// The stored bytes of a decimal written as text (`12.50`, `-1E-5`, `Infinity`, `NaN`), or why it cannot be.
pub fn from_string(text: &str) -> Result<[u8; 16], String> {
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let sign_bit = u128::from(negative) << 127;
    match body.to_ascii_lowercase().as_str() {
        "nan" => return Ok((0b11111u128 << 122).to_le_bytes()),
        "inf" | "infinity" => return Ok((sign_bit | (0b11110u128 << 122)).to_le_bytes()),
        _ => {}
    }
    let (mantissa, exp_text) = match body.find(['e', 'E']) {
        Some(at) => (body.get(..at).unwrap_or_default(), Some(body.get(at + 1..).unwrap_or_default())),
        None => (body, None),
    };
    let mut exponent: i64 = match exp_text {
        Some(e) => e.parse::<i64>().map_err(|_| format!("'{text}' is not a decimal number"))?,
        None => 0,
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if int.is_empty() && frac.is_empty() || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return Err(format!("'{text}' is not a decimal number"));
    }
    exponent -= i64::try_from(frac.len()).map_err(|_| "too many digits".to_owned())?;
    let digits: String = int.chars().chain(frac.chars()).collect();
    let mut significant = digits.trim_start_matches('0').to_owned();
    if significant.is_empty() {
        significant = "0".to_owned();
    }
    // Trailing zeros beyond 34 digits can be dropped into the exponent without changing the value.
    while significant.len() > MAX_DIGITS && significant.ends_with('0') {
        significant.pop();
        exponent += 1;
    }
    if significant.len() > MAX_DIGITS {
        return Err(format!("'{text}' has more than {MAX_DIGITS} significant digits"));
    }
    let coefficient: u128 = significant.parse().map_err(|_| format!("'{text}' is not a decimal number"))?;
    let exponent = i32::try_from(exponent).map_err(|_| format!("the exponent of '{text}' is out of range"))?;
    if !(MIN_EXPONENT..=MAX_EXPONENT).contains(&exponent) {
        return Err(format!("the exponent of '{text}' is out of range"));
    }
    let biased = u128::try_from(exponent + BIAS).map_err(|_| "exponent".to_owned())?;
    Ok((sign_bit | (biased << 113) | coefficient).to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_round_trip_through_their_string_form() {
        for text in [
            "0",
            "1",
            "-1",
            "1.5",
            "12.50",
            "0.001",
            "-0.000001",
            "1E+3",
            "1.23E+30",
            "0E-10",
            "9999999999999999999999999999999999",
            "1E-6176",
            "Infinity",
            "-Infinity",
            "NaN",
            "0.0000001",
        ] {
            let bytes = from_string(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            let shown = to_string(&bytes);
            assert_eq!(from_string(&shown), Ok(bytes), "{text} shown as {shown}");
        }
        assert_eq!(to_string(&from_string("12.50").expect("a decimal")), "12.50");
        assert_eq!(to_string(&from_string("0.000001").expect("a decimal")), "0.000001");
        assert_eq!(to_string(&from_string("0.0000001").expect("a decimal")), "1E-7");
        assert_eq!(to_string(&from_string("1000").expect("a decimal")), "1000");
        assert_eq!(to_string(&from_string("1E+3").expect("a decimal")), "1E+3");
        assert_eq!(to_string(&from_string("-0").expect("a decimal")), "-0");
    }

    #[test]
    fn bad_decimals_are_errors_not_panics() {
        for bad in ["", "-", ".", "1e", "e5", "1.2.3", "abc", "1E99999999999", "12345678901234567890123456789012345", "0x10"] {
            assert!(from_string(bad).is_err(), "{bad}");
        }
        for bytes in [[0xFF; 16], [0; 16], [0x7C; 16]] {
            let _ = to_string(&bytes);
        }
    }
}
