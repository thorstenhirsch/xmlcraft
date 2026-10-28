//! `float` and `double`: the lexical forms the schema language allows, and the values (IEEE 754 binary32 and binary64).

/// The shape of a floating-point lexical: an optional sign, digits with an optional point (at least one digit), an optional exponent;
/// or `INF`, `-INF`, `NaN`. (Not `+INF`, `inf`, `nan`, `Infinity`, hexadecimal or digit separators, which Rust would take.)
fn is_float_lexical(lexical: &str) -> bool {
    if matches!(lexical, "INF" | "-INF" | "NaN") {
        return true;
    }
    let rest = lexical.strip_prefix(['+', '-']).unwrap_or(lexical);
    let (mantissa, exponent) = match rest.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (rest, None),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !digits(int) || !digits(frac) || (int.is_empty() && frac.is_empty()) {
        return false;
    }
    match exponent {
        None => true,
        Some(e) => {
            let e = e.strip_prefix(['+', '-']).unwrap_or(e);
            !e.is_empty() && digits(e)
        }
    }
}

/// A `double` value. A number too large for the type is `INF` and one too small is zero (the nearest value).
pub fn parse_double(lexical: &str) -> Option<f64> {
    if !is_float_lexical(lexical) {
        return None;
    }
    match lexical {
        "INF" => Some(f64::INFINITY),
        "-INF" => Some(f64::NEG_INFINITY),
        "NaN" => Some(f64::NAN),
        _ => lexical.parse().ok(),
    }
}

/// A `float` value (binary32).
pub fn parse_float(lexical: &str) -> Option<f32> {
    if !is_float_lexical(lexical) {
        return None;
    }
    match lexical {
        "INF" => Some(f32::INFINITY),
        "-INF" => Some(f32::NEG_INFINITY),
        "NaN" => Some(f32::NAN),
        _ => lexical.parse().ok(),
    }
}

/// The canonical form of a `double`: `INF`, `-INF`, `NaN`, otherwise mantissa and exponent as the schema language writes them
/// (`1.5E2`, `0.0E0`).
pub fn double_canonical(value: f64) -> String {
    canonical(value.is_nan(), value.is_infinite(), value.is_sign_negative(), &format!("{value:e}"))
}

/// The canonical form of a `float`.
pub fn float_canonical(value: f32) -> String {
    canonical(value.is_nan(), value.is_infinite(), value.is_sign_negative(), &format!("{value:e}"))
}

fn canonical(nan: bool, infinite: bool, negative: bool, scientific: &str) -> String {
    if nan {
        return "NaN".to_owned();
    }
    if infinite {
        return if negative { "-INF" } else { "INF" }.to_owned();
    }
    // Rust writes `1.5e2` and `1e0`; the schema language wants `1.5E2` and `1.0E0`.
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((scientific, "0"));
    let mantissa = if mantissa.contains('.') { mantissa.to_owned() } else { format!("{mantissa}.0") };
    format!("{mantissa}E{exponent}")
}
