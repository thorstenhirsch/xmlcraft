//! The filters of the placeholders: dates, amounts, rates, numbers and units in the form of the template's language. Amounts keep the
//! digits the document wrote (an exact decimal, never a float): only the marks and the grouping change.

use xmlcraft_einvoice::model::{Decimal, Field};

use crate::template::{Filter, Lang};

/// `1234567` → `1.234.567` (de) or `1,234,567` (en).
fn group(whole: &str, sep: char) -> String {
    let digits: Vec<char> = whole.chars().collect();
    let mut out = String::with_capacity(digits.len().saturating_add(digits.len() / 3));
    for (i, c) in digits.iter().enumerate() {
        if i > 0 && (digits.len().saturating_sub(i)) % 3 == 0 {
            out.push(sep);
        }
        out.push(*c);
    }
    out
}

/// A decimal in the language's marks; `grouped` adds thousands separators.
pub fn decimal(d: Decimal, lang: Lang, grouped: bool) -> String {
    let (whole, fraction) = d.digits();
    let (thousands, mark) = match lang {
        Lang::De => ('.', ','),
        Lang::En => (',', '.'),
    };
    let whole = if grouped { group(&whole, thousands) } else { whole };
    let sign = if d.is_negative() { "-" } else { "" };
    if fraction.is_empty() { format!("{sign}{whole}") } else { format!("{sign}{whole}{mark}{fraction}") }
}

/// The short name of a common unit code (UN/ECE Recommendation 20), or the code.
fn unit_name(code: &str, lang: Lang) -> String {
    let (de, en) = match code {
        "C62" | "H87" | "EA" | "XPP" => ("Stk.", "pcs"),
        "HUR" => ("Std.", "h"),
        "DAY" => ("Tag(e)", "day(s)"),
        "MIN" => ("Min.", "min"),
        "MON" => ("Monat(e)", "month(s)"),
        "ANN" => ("Jahr(e)", "year(s)"),
        "WEE" => ("Woche(n)", "week(s)"),
        "KGM" => ("kg", "kg"),
        "GRM" => ("g", "g"),
        "TNE" => ("t", "t"),
        "MTR" => ("m", "m"),
        "KMT" => ("km", "km"),
        "MTK" => ("m²", "m²"),
        "MTQ" => ("m³", "m³"),
        "LTR" => ("l", "l"),
        "KWH" => ("kWh", "kWh"),
        "LS" => ("pauschal", "lump sum"),
        "SET" => ("Satz", "set"),
        "PR" => ("Paar", "pair"),
        _ => return code.to_owned(),
    };
    match lang {
        Lang::De => de.to_owned(),
        Lang::En => en.to_owned(),
    }
}

/// What a filter makes of a field. `currency` is the invoice currency, for an amount that names none. A value the filter cannot
/// read (a date that is no date, an amount that is no decimal) is shown as the document has it.
pub fn apply(filter: Option<Filter>, field: &Field, lang: Lang, currency: Option<&str>) -> String {
    match filter {
        None => field.text.clone(),
        Some(Filter::Upper) => field.text.to_uppercase(),
        Some(Filter::Date) => match field.date {
            Some(d) => match lang {
                Lang::De => d.format("%d.%m.%Y").to_string(),
                Lang::En => d.format("%Y-%m-%d").to_string(),
            },
            None => field.text.clone(),
        },
        Some(Filter::Money) => {
            let amount = field.decimal.map_or_else(|| field.text.clone(), |d| decimal(d, lang, true));
            match field.unit.as_deref().or(currency) {
                Some(c) => format!("{amount} {c}"),
                None => amount,
            }
        }
        Some(Filter::Percent) => {
            let n = field.decimal.map_or_else(|| field.text.clone(), |d| decimal(d, lang, false));
            match lang {
                Lang::De => format!("{n}\u{a0}%"),
                Lang::En => format!("{n}%"),
            }
        }
        Some(Filter::Number) => field.decimal.map_or_else(|| field.text.clone(), |d| decimal(d, lang, true)),
        Some(Filter::Unit) => unit_name(&field.text, lang),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(text: &str) -> Field {
        Field {
            text: text.to_owned(),
            range: 0..0,
            decimal: Decimal::parse(text),
            date: chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").ok(),
            unit: None,
        }
    }

    #[test]
    fn amounts_keep_their_digits_in_the_language_marks() {
        assert_eq!(apply(Some(Filter::Money), &field("1234567.50"), Lang::De, Some("EUR")), "1.234.567,50 EUR");
        assert_eq!(apply(Some(Filter::Money), &field("1234567.50"), Lang::En, Some("EUR")), "1,234,567.50 EUR");
        assert_eq!(apply(Some(Filter::Money), &field("-0.5"), Lang::De, None), "-0,5");
        assert_eq!(apply(Some(Filter::Money), &field("100"), Lang::De, Some("EUR")), "100 EUR");
        assert_eq!(apply(Some(Filter::Money), &field("999"), Lang::En, None), "999");
        assert_eq!(apply(Some(Filter::Money), &field("1000.000"), Lang::En, None), "1,000.000", "the document's decimals stay");
        assert_eq!(apply(Some(Filter::Money), &field("12,50"), Lang::De, Some("EUR")), "12,50 EUR", "not a decimal: as written");
    }

    #[test]
    fn dates_rates_numbers_units_and_upper_case() {
        assert_eq!(apply(Some(Filter::Date), &field("2025-12-31"), Lang::De, None), "31.12.2025");
        assert_eq!(apply(Some(Filter::Date), &field("2025-12-31"), Lang::En, None), "2025-12-31");
        assert_eq!(apply(Some(Filter::Date), &field("soon"), Lang::De, None), "soon");
        assert_eq!(apply(Some(Filter::Percent), &field("19"), Lang::De, None), "19\u{a0}%");
        assert_eq!(apply(Some(Filter::Percent), &field("7.5"), Lang::De, None), "7,5\u{a0}%");
        assert_eq!(apply(Some(Filter::Percent), &field("7.5"), Lang::En, None), "7.5%");
        assert_eq!(apply(Some(Filter::Number), &field("1500.25"), Lang::De, None), "1.500,25");
        assert_eq!(apply(Some(Filter::Unit), &field("HUR"), Lang::De, None), "Std.");
        assert_eq!(apply(Some(Filter::Unit), &field("XYZ"), Lang::En, None), "XYZ");
        assert_eq!(apply(Some(Filter::Upper), &field("de123"), Lang::En, None), "DE123");
        assert_eq!(apply(None, &field("as is"), Lang::En, None), "as is");
        assert_eq!(group("", '.'), "");
        assert_eq!(group("123", '.'), "123");
        assert_eq!(group("1234", '.'), "1.234");
    }
}
