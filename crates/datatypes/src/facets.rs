//! The facets of one derivation step, and what each of them says about a value (XML Schema Part 2, chapter 4).

use std::cmp::Ordering;

use xmlcraft_pattern::Pattern;

use crate::decimal::Decimal;
use crate::error::{Violation, ViolationKind};
use crate::value::Value;
use crate::whitespace::Whitespace;

/// The facets one restriction sets. (A type's effective facets are those of all the steps from the primitive type to it; a value is
/// checked against every step, which is the same thing for a valid derivation.)
#[derive(Clone, Debug, Default)]
pub struct Facets {
    pub length: Option<usize>,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    /// The patterns of this step: a value has to match one of them. (Steps are combined with "and".)
    pub patterns: Vec<Pattern>,
    pub enumeration: Option<Vec<Value>>,
    pub white_space: Option<Whitespace>,
    pub max_inclusive: Option<Value>,
    pub max_exclusive: Option<Value>,
    pub min_exclusive: Option<Value>,
    pub min_inclusive: Option<Value>,
    pub total_digits: Option<usize>,
    pub fraction_digits: Option<usize>,
    /// The facets that may not be changed by a further restriction.
    pub fixed: Fixed,
}

/// Which facets are `fixed`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fixed {
    pub length: bool,
    pub min_length: bool,
    pub max_length: bool,
    pub white_space: bool,
    pub max_inclusive: bool,
    pub max_exclusive: bool,
    pub min_exclusive: bool,
    pub min_inclusive: bool,
    pub total_digits: bool,
    pub fraction_digits: bool,
}

impl Facets {
    /// Whether nothing is set.
    pub fn is_empty(&self) -> bool {
        self.length.is_none()
            && self.min_length.is_none()
            && self.max_length.is_none()
            && self.patterns.is_empty()
            && self.enumeration.is_none()
            && self.white_space.is_none()
            && self.max_inclusive.is_none()
            && self.max_exclusive.is_none()
            && self.min_exclusive.is_none()
            && self.min_inclusive.is_none()
            && self.total_digits.is_none()
            && self.fraction_digits.is_none()
    }

    /// Whether `lexical` (already normalised) matches one of the patterns of this step, if it has any.
    pub fn check_patterns(&self, lexical: &str) -> Result<(), Violation> {
        if self.patterns.is_empty() || self.patterns.iter().any(|p| p.is_match(lexical)) {
            return Ok(());
        }
        let shown = self.patterns.iter().map(|p| format!("'{}'", p.as_str())).collect::<Vec<_>>().join(" or ");
        Err(Violation::new(ViolationKind::Pattern, format!("'{lexical}' does not match the pattern {shown}")))
    }

    /// Check `value` against the facets of this step that look at values.
    pub fn check_value(&self, value: &Value) -> Result<(), Violation> {
        if let Some(allowed) = &self.enumeration
            && !allowed.iter().any(|a| a.equals(value))
        {
            let list = allowed.iter().map(|a| format!("'{}'", a.canonical())).collect::<Vec<_>>().join(", ");
            return Err(Violation::new(ViolationKind::Enumeration, format!("'{}' is not one of {list}", value.canonical())));
        }
        if let Some(length) = value.length() {
            if let Some(n) = self.length
                && length != n
            {
                return Err(Violation::new(ViolationKind::Length, format!("the length is {length}, it must be {n}")));
            }
            if let Some(n) = self.min_length
                && length < n
            {
                return Err(Violation::new(ViolationKind::MinLength, format!("the length is {length}, it must be at least {n}")));
            }
            if let Some(n) = self.max_length
                && length > n
            {
                return Err(Violation::new(ViolationKind::MaxLength, format!("the length is {length}, it must be at most {n}")));
            }
        }
        let bound = |bound: &Option<Value>, kind: ViolationKind, ok: &dyn Fn(Ordering) -> bool, what: &str| -> Result<(), Violation> {
            let Some(limit) = bound else { return Ok(()) };
            match value.compare(limit) {
                Some(order) if ok(order) => Ok(()),
                Some(_) => Err(Violation::new(kind, format!("'{}' must be {what} '{}'", value.canonical(), limit.canonical()))),
                None => Err(Violation::new(kind, format!("'{}' cannot be compared with '{}'", value.canonical(), limit.canonical()))),
            }
        };
        bound(&self.min_inclusive, ViolationKind::MinInclusive, &|o| o != Ordering::Less, "at least")?;
        bound(&self.min_exclusive, ViolationKind::MinExclusive, &|o| o == Ordering::Greater, "greater than")?;
        bound(&self.max_inclusive, ViolationKind::MaxInclusive, &|o| o != Ordering::Greater, "at most")?;
        bound(&self.max_exclusive, ViolationKind::MaxExclusive, &|o| o == Ordering::Less, "less than")?;
        if let Value::Decimal(number) = value {
            check_digits(number, self.total_digits, self.fraction_digits)?;
        }
        Ok(())
    }
}

fn check_digits(number: &Decimal, total: Option<usize>, fraction: Option<usize>) -> Result<(), Violation> {
    if let Some(n) = total
        && number.total_digits() > n
    {
        return Err(Violation::new(ViolationKind::TotalDigits, format!("'{number}' has {} digits, at most {n} are allowed", number.total_digits())));
    }
    if let Some(n) = fraction
        && number.fraction_digits() > n
    {
        return Err(Violation::new(
            ViolationKind::FractionDigits,
            format!("'{number}' has {} fraction digits, at most {n} are allowed", number.fraction_digits()),
        ));
    }
    Ok(())
}
