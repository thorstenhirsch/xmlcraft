//! The calculation check of XBRL 2.1 (section 5.2.5.2): for each summation item of a calculation linkbase, the facts that are
//! C-equal and U-equal to it (the same context and unit by value, not by id) and whose concept contributes are summed, each rounded to
//! its own decimals and multiplied by the weight of its arc; the total rounded to the decimals of the summation item must equal the
//! summation item. Only what the instance says explicitly is used (no inferred facts); a calculation binds only for a summation item
//! that is not nil, with at least one contributing item that is not nil, and no contributing concept twice.
//!
//! Numbers are exact decimals (`i128` and a power of ten), with ties rounded to even as the specification's 2011 revision says; a
//! value that is not a plain number, or that does not fit, leaves its calculation unchecked rather than wrong.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::taxonomy::{Calculation, Taxonomy};
use crate::{Context, Fact, Finding, Instance, Level, Period, Unit};

/// The most facts a calculation check looks at (more are not checked, which is said by nothing: they are too many to hold).
const MAX_FACTS: usize = 1_000_000;
/// The most digits of a number.
const MAX_DIGITS: usize = 30;
/// The largest power of ten used.
const MAX_POWER: u32 = 36;

/// An exact decimal: `mant` times ten to the power of minus `scale`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Dec {
    mant: i128,
    scale: i32,
}

/// How exactly a fact says its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decimals {
    /// Exactly (`INF`).
    Exact,
    /// To this decimal place (negative: to tens, hundreds …).
    At(i32),
}

fn pow10(n: u32) -> Option<i128> {
    if n > MAX_POWER { None } else { 10i128.checked_pow(n) }
}

impl Dec {
    /// A plain decimal number (`-12.50`, `1e3`); none for anything else.
    pub(crate) fn parse(text: &str) -> Option<Dec> {
        let t = text.trim();
        let (negative, t) = match t.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, t.strip_prefix('+').unwrap_or(t)),
        };
        let (number, exponent) = match t.split_once(['e', 'E']) {
            Some((n, e)) => (n, e.parse::<i32>().ok().filter(|e| e.abs() <= 60)?),
            None => (t, 0),
        };
        let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
        if whole.is_empty() && fraction.is_empty() || !whole.bytes().chain(fraction.bytes()).all(|b| b.is_ascii_digit()) {
            return None;
        }
        let digits = format!("{whole}{fraction}");
        let trimmed = digits.trim_start_matches('0');
        if trimmed.len() > MAX_DIGITS {
            return None;
        }
        let mant: i128 = if trimmed.is_empty() { 0 } else { trimmed.parse().ok()? };
        let scale = i32::try_from(fraction.len()).ok()?.checked_sub(exponent)?;
        Some(Dec { mant: if negative { -mant } else { mant }, scale })
    }

    fn is_zero(self) -> bool {
        self.mant == 0
    }

    /// The number of decimal digits of the magnitude minus the places after the point: where the first digit is (`1200` is 4, `0.05` is -1).
    fn magnitude(self) -> i32 {
        let digits = i32::try_from(self.mant.unsigned_abs().to_string().len()).unwrap_or(i32::MAX);
        digits.saturating_sub(self.scale)
    }

    /// Rounded to the decimal place `places` (ties to even).
    pub(crate) fn round_to(self, places: Decimals) -> Option<Dec> {
        let Decimals::At(d) = places else { return Some(self) };
        if self.scale <= d {
            return Some(self);
        }
        let drop = u32::try_from(self.scale.checked_sub(d)?).ok()?;
        let Some(p) = pow10(drop) else { return Some(Dec { mant: 0, scale: d }) };
        let (q, r) = (self.mant / p, self.mant % p);
        let twice = r.unsigned_abs().checked_mul(2)?;
        let half = p.unsigned_abs();
        let away = match twice.cmp(&half) {
            Ordering::Greater => true,
            Ordering::Equal => q % 2 != 0,
            Ordering::Less => false,
        };
        let q = if away { q.checked_add(if self.mant < 0 { -1 } else { 1 })? } else { q };
        Some(Dec { mant: q, scale: d })
    }

    fn aligned(self, other: Dec) -> Option<(i128, i128, i32)> {
        let scale = self.scale.max(other.scale);
        let lift = |d: Dec| -> Option<i128> { d.mant.checked_mul(pow10(u32::try_from(scale.checked_sub(d.scale)?).ok()?)?) };
        Some((lift(self)?, lift(other)?, scale))
    }

    fn plus(self, other: Dec) -> Option<Dec> {
        let (a, b, scale) = self.aligned(other)?;
        Some(Dec { mant: a.checked_add(b)?, scale })
    }

    fn times(self, other: Dec) -> Option<Dec> {
        Some(Dec { mant: self.mant.checked_mul(other.mant)?, scale: self.scale.checked_add(other.scale)? })
    }

    pub(crate) fn equals(self, other: Dec) -> bool {
        self.aligned(other).is_some_and(|(a, b, _)| a == b)
    }

    pub(crate) fn text(self) -> String {
        if self.scale <= 0 {
            let zeros = usize::try_from(self.scale.unsigned_abs()).unwrap_or(0).min(80);
            return format!("{}{}", self.mant, "0".repeat(zeros));
        }
        let scale = usize::try_from(self.scale).unwrap_or(0).min(80);
        let digits = self.mant.unsigned_abs().to_string();
        let padded = format!("{digits:0>width$}", width = scale + 1);
        let split = padded.len().saturating_sub(scale);
        let (whole, fraction) = padded.split_at(split);
        format!("{}{whole}.{fraction}", if self.mant < 0 { "-" } else { "" })
    }
}

/// How a fact says its number: its `decimals`, or what its `precision` comes to. `Err(())` for a precision of 0 (which makes any
/// calculation it is in inconsistent); none when it says nothing usable.
fn decimals_of(fact: &Fact, value: Dec) -> Option<Result<Decimals, ()>> {
    if let Some(d) = fact.decimals.as_deref().map(str::trim) {
        return if d == "INF" { Some(Ok(Decimals::Exact)) } else { d.parse::<i32>().ok().map(|n| Ok(Decimals::At(n))) };
    }
    let p = fact.precision.as_deref().map(str::trim)?;
    if p == "INF" {
        return Some(Ok(Decimals::Exact));
    }
    let p: i32 = p.parse().ok()?;
    match p {
        0 => Some(Err(())),
        p if p < 0 => None,
        _ if value.is_zero() => Some(Ok(Decimals::Exact)),
        p => Some(Ok(Decimals::At(p.checked_sub(value.magnitude())?))),
    }
}

/// The value of a context for comparing: entity, period and dimensions, not the id.
fn context_key(c: &Context) -> String {
    let period = match &c.period {
        Some(Period::Instant(d)) => format!("i{d}"),
        Some(Period::Duration(a, b)) => format!("d{a}/{b}"),
        Some(Period::Forever) => "f".to_owned(),
        None => "-".to_owned(),
    };
    let mut dims: Vec<String> =
        c.dimensions.iter().map(|d| format!("{}={}{}{}", d.dimension, d.member, u8::from(d.typed), u8::from(d.in_scenario))).collect();
    dims.sort_unstable();
    format!("{}|{}|{period}|{}", c.entity_scheme, c.entity, dims.join(";"))
}

fn unit_key(u: &Unit) -> &str {
    &u.measure
}

/// The facts that can be in a calculation (numbers, not nil, with a context and a unit), by (context, unit) as values and then by concept.
type Groups<'a> = HashMap<(&'a str, &'a str), HashMap<&'a str, Vec<&'a Fact>>>;

/// What the calculation linkbases of the taxonomy say of the facts of `instance` that they do not add up to.
pub fn check_calculations(instance: &Instance, taxonomy: &Taxonomy) -> Vec<Finding> {
    if taxonomy.calculations.is_empty() || instance.facts.len() > MAX_FACTS {
        return Vec::new();
    }
    let contexts: HashMap<&str, String> = instance.contexts.iter().map(|c| (c.id.as_str(), context_key(c))).collect();
    let units: HashMap<&str, &str> = instance.units.iter().map(|u| (u.id.as_str(), unit_key(u))).collect();
    let mut groups: Groups = HashMap::new();
    for fact in instance.facts.iter().filter(|f| !f.nil) {
        let (Some(c), Some(u)) = (fact.context_ref.as_deref(), fact.unit_ref.as_deref()) else { continue };
        let (Some(c), Some(u)) = (contexts.get(c), units.get(u)) else { continue };
        groups.entry((c.as_str(), u)).or_default().entry(fact.concept.as_str()).or_default().push(fact);
    }
    let mut found = Vec::new();
    for calculation in &taxonomy.calculations {
        for group in groups.values() {
            let Some(sums) = group.get(calculation.parent.as_str()) else { continue };
            // (Two summation items in one context and unit are duplicates: the calculation is left to the consistency check of facts.)
            let [sum] = sums.as_slice() else { continue };
            if let Some(finding) = check_one(calculation, sum, group) {
                found.push(finding);
            }
        }
    }
    found.sort_by_key(|f| f.span.start);
    found
}

/// One calculation for one summation item; a finding when it binds and is not consistent.
fn check_one(calculation: &Calculation, sum: &Fact, group: &HashMap<&str, Vec<&Fact>>) -> Option<Finding> {
    let sum_value = Dec::parse(&sum.value)?;
    let mut contributing: Vec<(&Fact, Dec, crate::taxonomy::Contribution)> = Vec::new();
    for item in &calculation.items {
        let Some(facts) = group.get(item.concept.as_str()) else { continue };
        // A contributing concept twice (duplicates): the calculation does not bind.
        let [fact] = facts.as_slice() else { return None };
        if contributing.iter().any(|(f, _, _)| f.concept == fact.concept) {
            continue;
        }
        contributing.push((fact, Dec::parse(&fact.value)?, item.clone()));
    }
    if contributing.is_empty() {
        return None;
    }
    let level = Level::Warning;
    let make = |message: String| Finding { code: "xbrl.calculation-inconsistent", level, message, span: sum.span.clone() };
    let roles = &calculation.role;
    // A precision of 0 anywhere makes it inconsistent.
    let mut total = Dec { mant: 0, scale: 0 };
    let mut zero_precision = decimals_of(sum, sum_value)? == Err(());
    for (fact, value, item) in &contributing {
        match decimals_of(fact, *value)? {
            Ok(places) => {
                let rounded = value.round_to(places)?;
                total = total.plus(rounded.times(Dec::parse(&item.weight)?)?)?;
            }
            Err(()) => zero_precision = true,
        }
    }
    if zero_precision {
        return Some(make(format!("The calculation of {} in {roles} is inconsistent: an item has a precision of 0.", sum.name)));
    }
    let places = decimals_of(sum, sum_value)?.ok()?;
    let (expected, given) = (total.round_to(places)?, sum_value.round_to(places)?);
    (!expected.equals(given)).then(|| {
        make(format!(
            "The calculation of {} in {roles} is inconsistent: the fact is {} and its {} contributing item{} add up to {}.",
            sum.name,
            given.text(),
            contributing.len(),
            if contributing.len() == 1 { "" } else { "s" },
            expected.text()
        ))
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn d(text: &str) -> Dec {
        Dec::parse(text).expect("a number")
    }

    #[test]
    fn numbers_are_exact_and_rounding_goes_to_even() {
        assert_eq!(d("12.50").text(), "12.50");
        assert_eq!(d("-0.05").text(), "-0.05");
        assert_eq!(d("1e3").text(), "1000");
        assert_eq!(d("1.5E-2").text(), "0.015");
        assert_eq!(d("2.5").round_to(Decimals::At(0)).map(Dec::text).as_deref(), Some("2"));
        assert_eq!(d("3.5").round_to(Decimals::At(0)).map(Dec::text).as_deref(), Some("4"));
        assert_eq!(d("-2.5").round_to(Decimals::At(0)).map(Dec::text).as_deref(), Some("-2"));
        assert_eq!(d("2.51").round_to(Decimals::At(0)).map(Dec::text).as_deref(), Some("3"));
        assert_eq!(d("1250").round_to(Decimals::At(-2)).map(Dec::text).as_deref(), Some("1200"));
        assert_eq!(d("1350").round_to(Decimals::At(-2)).map(Dec::text).as_deref(), Some("1400"));
        assert_eq!(d("1.234").round_to(Decimals::Exact), Some(d("1.234")));
        assert!(d("0.30").equals(d("0.3")) && !d("0.3").equals(d("0.31")));
        assert_eq!(d("0.1").plus(d("0.2")).map(Dec::text).as_deref(), Some("0.3"), "no binary fractions");
        for bad in ["", ".", "abc", "1.2.3", "INF", "NaN", "1e999", "- 1", &"9".repeat(40)] {
            assert_eq!(Dec::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_decimals_of_a_fact_are_its_decimals_or_what_its_precision_comes_to() {
        let fact = |decimals: Option<&str>, precision: Option<&str>| Fact {
            concept: "{urn:x}A".into(),
            name: "x:A".into(),
            value: String::new(),
            context_ref: None,
            unit_ref: None,
            decimals: decimals.map(str::to_owned),
            precision: precision.map(str::to_owned),
            nil: false,
            id: None,
            span: 0..0,
            value_problem: None,
        };
        assert_eq!(decimals_of(&fact(Some("-3"), None), d("1")), Some(Ok(Decimals::At(-3))));
        assert_eq!(decimals_of(&fact(Some("INF"), None), d("1")), Some(Ok(Decimals::Exact)));
        // 1234 with a precision of 2 is 1200: decimals -2. 0.05 with a precision of 1 is 0.05: decimals 2.
        assert_eq!(decimals_of(&fact(None, Some("2")), d("1234")), Some(Ok(Decimals::At(-2))));
        assert_eq!(decimals_of(&fact(None, Some("1")), d("0.05")), Some(Ok(Decimals::At(2))));
        assert_eq!(decimals_of(&fact(None, Some("0")), d("5")), Some(Err(())));
        assert_eq!(decimals_of(&fact(None, None), d("5")), None);
        assert_eq!(decimals_of(&fact(Some("x"), None), d("5")), None);
    }
}
