//! Making new simple types: by restriction (facets), as a list, as a union, with the constraints on facets that a schema must meet
//! (XML Schema Part 2, chapter 4, "Constraints on XML Representation" and "Schema Component Constraint: ... Valid Restriction").

use std::cmp::Ordering;
use std::sync::Arc;

use xmlcraft_pattern::Pattern;

use crate::decimal::Decimal;
use crate::facets::{Facets, Fixed};
use crate::types::{Derivation, ExpandedName, NamespaceContext, Primitive, SimpleType, Variety};
use crate::value::Value;
use crate::whitespace::Whitespace;

/// A facet as a schema writes it: the value and whether it is `fixed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setting {
    pub value: String,
    pub fixed: bool,
}

impl Setting {
    pub fn new(value: &str) -> Self {
        Self { value: value.to_owned(), fixed: false }
    }

    pub fn fixed(value: &str) -> Self {
        Self { value: value.to_owned(), fixed: true }
    }
}

/// The facets of one `xs:restriction`, as written.
#[derive(Clone, Debug, Default)]
pub struct FacetSpec {
    pub length: Option<Setting>,
    pub min_length: Option<Setting>,
    pub max_length: Option<Setting>,
    pub total_digits: Option<Setting>,
    pub fraction_digits: Option<Setting>,
    pub white_space: Option<Setting>,
    pub max_inclusive: Option<Setting>,
    pub max_exclusive: Option<Setting>,
    pub min_exclusive: Option<Setting>,
    pub min_inclusive: Option<Setting>,
    pub patterns: Vec<String>,
    pub enumeration: Vec<String>,
}

/// The facets in effect for a type: the last step that sets each one.
#[derive(Default)]
struct Effective {
    length: Option<(usize, bool)>,
    min_length: Option<(usize, bool)>,
    max_length: Option<(usize, bool)>,
    total_digits: Option<(usize, bool)>,
    fraction_digits: Option<(usize, bool)>,
    white_space: Option<(Whitespace, bool)>,
    max_inclusive: Option<(Value, bool)>,
    max_exclusive: Option<(Value, bool)>,
    min_exclusive: Option<(Value, bool)>,
    min_inclusive: Option<(Value, bool)>,
}

fn effective(t: &SimpleType) -> Effective {
    let steps = t.steps();
    // The most derived step that sets a facet decides; `fixed` is the flag of that step.
    let find = |get: &dyn Fn(&Facets) -> bool| steps.iter().find(|s| get(&s.facets)).copied();
    let number = |get: fn(&Facets) -> Option<usize>, fixed: fn(&Fixed) -> bool| {
        find(&|f| get(f).is_some()).and_then(|s| get(&s.facets).map(|n| (n, fixed(&s.facets.fixed))))
    };
    let bound = |get: fn(&Facets) -> Option<&Value>, fixed: fn(&Fixed) -> bool| {
        find(&|f| get(f).is_some()).and_then(|s| get(&s.facets).map(|v| (v.clone(), fixed(&s.facets.fixed))))
    };
    Effective {
        length: number(|f| f.length, |x| x.length),
        min_length: number(|f| f.min_length, |x| x.min_length),
        max_length: number(|f| f.max_length, |x| x.max_length),
        total_digits: number(|f| f.total_digits, |x| x.total_digits),
        fraction_digits: number(|f| f.fraction_digits, |x| x.fraction_digits),
        white_space: find(&|f| f.white_space.is_some()).and_then(|s| s.facets.white_space.map(|w| (w, s.facets.fixed.white_space))),
        max_inclusive: bound(|f| f.max_inclusive.as_ref(), |x| x.max_inclusive),
        max_exclusive: bound(|f| f.max_exclusive.as_ref(), |x| x.max_exclusive),
        min_exclusive: bound(|f| f.min_exclusive.as_ref(), |x| x.min_exclusive),
        min_inclusive: bound(|f| f.min_inclusive.as_ref(), |x| x.min_inclusive),
    }
}

/// Which facets apply to what: the length facets to strings, binary data, names, URIs and lists; the bounds to numbers, durations and
/// dates and times; the digits to decimals; the patterns, the enumeration and the white space to every atomic type, the patterns and
/// the enumeration to unions.
struct Applies {
    length: bool,
    bounds: bool,
    digits: bool,
    white_space: bool,
}

fn applies(variety: &Variety) -> Applies {
    match variety {
        Variety::Atomic(p) => Applies {
            length: matches!(
                p,
                Primitive::AnySimple
                    | Primitive::String
                    | Primitive::AnyUri
                    | Primitive::HexBinary
                    | Primitive::Base64Binary
                    | Primitive::QName
                    | Primitive::Notation
            ),
            bounds: matches!(
                p,
                Primitive::Decimal
                    | Primitive::Float
                    | Primitive::Double
                    | Primitive::Duration
                    | Primitive::DateTime
                    | Primitive::Time
                    | Primitive::Date
                    | Primitive::GYearMonth
                    | Primitive::GYear
                    | Primitive::GMonthDay
                    | Primitive::GDay
                    | Primitive::GMonth
            ),
            digits: *p == Primitive::Decimal,
            white_space: true,
        },
        Variety::List(_) => Applies { length: true, bounds: false, digits: false, white_space: true },
        Variety::Union(_) => Applies { length: false, bounds: false, digits: false, white_space: false },
    }
}

fn count(setting: &Setting, name: &str, minimum: usize, errors: &mut Vec<String>) -> Option<usize> {
    let parsed =
        Decimal::parse_integer(setting.value.trim()).and_then(|d| d.to_i128()).and_then(|n| usize::try_from(n).ok()).filter(|n| *n >= minimum);
    if parsed.is_none() {
        let kind = if minimum == 0 { "a non-negative integer" } else { "a positive integer" };
        errors.push(format!("{name}: '{}' is not {kind}", setting.value));
    }
    parsed
}

/// Make the type that restricts `base` by the facets in `spec`. The errors are the schema component constraints that are broken.
/// `ns` is where the values of the facets are read (for the types whose values have a qualified name).
pub fn restrict(
    base: &Arc<SimpleType>,
    name: Option<ExpandedName>,
    spec: &FacetSpec,
    ns: &dyn NamespaceContext,
) -> Result<Arc<SimpleType>, Vec<String>> {
    let mut errors = Vec::new();
    let allowed = applies(&base.variety);
    let basis = effective(base);
    let mut facets = Facets::default();
    let refuse = |facet: &str, errors: &mut Vec<String>| errors.push(format!("{facet} does not apply to {}", describe(base)));

    // The numbers.
    let number = |setting: &Option<Setting>, facet: &str, minimum: usize, applicable: bool, errors: &mut Vec<String>| -> Option<(usize, bool)> {
        let setting = setting.as_ref()?;
        if !applicable {
            refuse(facet, errors);
            return None;
        }
        count(setting, facet, minimum, errors).map(|n| (n, setting.fixed))
    };
    let length = number(&spec.length, "length", 0, allowed.length, &mut errors);
    let min_length = number(&spec.min_length, "minLength", 0, allowed.length, &mut errors);
    let max_length = number(&spec.max_length, "maxLength", 0, allowed.length, &mut errors);
    let total_digits = number(&spec.total_digits, "totalDigits", 1, allowed.digits, &mut errors);
    let fraction_digits = number(&spec.fraction_digits, "fractionDigits", 0, allowed.digits, &mut errors);
    (facets.length, facets.fixed.length) = split(length);
    (facets.min_length, facets.fixed.min_length) = split(min_length);
    (facets.max_length, facets.fixed.max_length) = split(max_length);
    (facets.total_digits, facets.fixed.total_digits) = split(total_digits);
    (facets.fraction_digits, facets.fixed.fraction_digits) = split(fraction_digits);

    // White space.
    if let Some(setting) = &spec.white_space {
        if !allowed.white_space {
            refuse("whiteSpace", &mut errors);
        } else {
            match Whitespace::from_name(setting.value.trim()) {
                Some(w) => {
                    facets.white_space = Some(w);
                    facets.fixed.white_space = setting.fixed;
                    let current = basis.white_space.map_or_else(|| base.whitespace(), |(w, _)| w);
                    if w.strictness() < current.strictness() {
                        errors.push(format!("whiteSpace: '{}' is less strict than the base type's '{}'", w.name(), current.name()));
                    }
                }
                None => errors.push(format!("whiteSpace: '{}' is not preserve, replace or collapse", setting.value)),
            }
        }
    }

    // The bounds and the enumeration are values of the base type.
    let read = |setting: &Option<Setting>, facet: &str, errors: &mut Vec<String>| -> Option<(Value, bool)> {
        let setting = setting.as_ref()?;
        if !allowed.bounds {
            refuse(facet, errors);
            return None;
        }
        match base.validate(&setting.value, ns) {
            Ok(v) => Some((v, setting.fixed)),
            Err(e) => {
                errors.push(format!("{facet}: '{}' is not a valid value of the base type: {}", setting.value, e.message));
                None
            }
        }
    };
    let max_inclusive = read(&spec.max_inclusive, "maxInclusive", &mut errors);
    let max_exclusive = read(&spec.max_exclusive, "maxExclusive", &mut errors);
    let min_exclusive = read(&spec.min_exclusive, "minExclusive", &mut errors);
    let min_inclusive = read(&spec.min_inclusive, "minInclusive", &mut errors);
    (facets.max_inclusive, facets.fixed.max_inclusive) = split(max_inclusive.clone());
    (facets.max_exclusive, facets.fixed.max_exclusive) = split(max_exclusive.clone());
    (facets.min_exclusive, facets.fixed.min_exclusive) = split(min_exclusive.clone());
    (facets.min_inclusive, facets.fixed.min_inclusive) = split(min_inclusive.clone());
    if !spec.enumeration.is_empty() {
        let mut values = Vec::new();
        for lexical in &spec.enumeration {
            match base.validate(lexical, ns) {
                Ok(v) => values.push(v),
                Err(e) => errors.push(format!("enumeration: '{lexical}' is not a valid value of the base type: {}", e.message)),
            }
        }
        facets.enumeration = Some(values);
    }
    for source in &spec.patterns {
        match Pattern::new(source) {
            Ok(p) => facets.patterns.push(p),
            Err(e) => errors.push(format!("pattern '{source}': {e}")),
        }
    }

    // Constraints between the facets, with the base type's.
    fixed_unchanged(&mut errors, "length", length, basis.length);
    fixed_unchanged(&mut errors, "minLength", min_length, basis.min_length);
    fixed_unchanged(&mut errors, "maxLength", max_length, basis.max_length);
    fixed_unchanged(&mut errors, "totalDigits", total_digits, basis.total_digits);
    fixed_unchanged(&mut errors, "fractionDigits", fraction_digits, basis.fraction_digits);
    if let (Some(now), Some((was, true))) = (facets.white_space, basis.white_space)
        && now != was
    {
        errors.push(format!("whiteSpace: the base type fixes it to '{}'", was.name()));
    }
    for (facet, now, was) in [
        ("maxInclusive", &max_inclusive, &basis.max_inclusive),
        ("maxExclusive", &max_exclusive, &basis.max_exclusive),
        ("minExclusive", &min_exclusive, &basis.min_exclusive),
        ("minInclusive", &min_inclusive, &basis.min_inclusive),
    ] {
        if let (Some((now, _)), Some((was, true))) = (now, was)
            && !now.equals(was)
        {
            errors.push(format!("{facet}: the base type fixes it to '{}'", was.canonical()));
        }
    }
    let own_or_base = |own: Option<(usize, bool)>, base_value: Option<(usize, bool)>| own.or(base_value).map(|(n, _)| n);
    let (len, min, max) = (own_or_base(length, basis.length), own_or_base(min_length, basis.min_length), own_or_base(max_length, basis.max_length));
    if let (Some(min), Some(max)) = (min, max)
        && min > max
    {
        errors.push(format!("minLength {min} is greater than maxLength {max}"));
    }
    if let Some(len) = len {
        if min.is_some_and(|m| len < m) {
            errors.push(format!("length {len} is less than minLength {}", min.unwrap_or(0)));
        }
        if max.is_some_and(|m| len > m) {
            errors.push(format!("length {len} is greater than maxLength {}", max.unwrap_or(0)));
        }
    }
    if let (Some((now, _)), Some((was, _))) = (min_length, basis.min_length)
        && now < was
    {
        errors.push(format!("minLength {now} is less than the base type's {was}"));
    }
    if let (Some((now, _)), Some((was, _))) = (max_length, basis.max_length)
        && now > was
    {
        errors.push(format!("maxLength {now} is greater than the base type's {was}"));
    }
    if let (Some((now, _)), Some((was, _))) = (length, basis.length)
        && now != was
    {
        errors.push(format!("length {now} is not the base type's {was}"));
    }
    let total = own_or_base(total_digits, basis.total_digits);
    let fraction = own_or_base(fraction_digits, basis.fraction_digits);
    if let (Some(total), Some(fraction)) = (total, fraction)
        && fraction > total
    {
        errors.push(format!("fractionDigits {fraction} is greater than totalDigits {total}"));
    }
    if let (Some((now, _)), Some((was, _))) = (total_digits, basis.total_digits)
        && now > was
    {
        errors.push(format!("totalDigits {now} is greater than the base type's {was}"));
    }
    if let (Some((now, _)), Some((was, _))) = (fraction_digits, basis.fraction_digits)
        && now > was
    {
        errors.push(format!("fractionDigits {now} is greater than the base type's {was}"));
    }
    if max_inclusive.is_some() && max_exclusive.is_some() {
        errors.push("maxInclusive and maxExclusive cannot both be given".to_owned());
    }
    if min_inclusive.is_some() && min_exclusive.is_some() {
        errors.push("minInclusive and minExclusive cannot both be given".to_owned());
    }
    // The range that is left must not be empty.
    let upper = max_inclusive.as_ref().map(|(v, _)| (v, true)).or(max_exclusive.as_ref().map(|(v, _)| (v, false)));
    let lower = min_inclusive.as_ref().map(|(v, _)| (v, true)).or(min_exclusive.as_ref().map(|(v, _)| (v, false)));
    if let (Some((low, low_inclusive)), Some((high, high_inclusive))) = (lower, upper) {
        match low.compare(high) {
            Some(Ordering::Greater) => {
                errors.push(format!("the lower bound {} is greater than the upper bound {}", low.canonical(), high.canonical()))
            }
            Some(Ordering::Equal) if !(low_inclusive && high_inclusive) => {
                errors.push(format!("the bounds {} and {} leave no value", low.canonical(), high.canonical()))
            }
            None => errors.push(format!("the bounds {} and {} cannot be compared", low.canonical(), high.canonical())),
            _ => {}
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Arc::new(SimpleType { name, variety: base.variety.clone(), derivation: Derivation::Restriction, base: Some(base.clone()), facets }))
}

fn split<T>(value: Option<(T, bool)>) -> (Option<T>, bool) {
    match value {
        Some((v, fixed)) => (Some(v), fixed),
        None => (None, false),
    }
}

fn fixed_unchanged(errors: &mut Vec<String>, facet: &str, now: Option<(usize, bool)>, was: Option<(usize, bool)>) {
    if let (Some((now, _)), Some((was, true))) = (now, was)
        && now != was
    {
        errors.push(format!("{facet}: the base type fixes it to {was}"));
    }
}

fn describe(t: &SimpleType) -> String {
    t.name().map_or_else(|| "the base type".to_owned(), |n| n.local.clone())
}

/// A list type of `item`s. The item type must be atomic, or a union of atomic types (a list does not hold lists).
pub fn list_of(item: &Arc<SimpleType>, name: Option<ExpandedName>) -> Result<Arc<SimpleType>, Vec<String>> {
    let holds_a_list = match &item.variety {
        Variety::List(_) => true,
        Variety::Union(members) => members.iter().any(|m| matches!(m.variety, Variety::List(_))),
        Variety::Atomic(_) => false,
    };
    if holds_a_list {
        return Err(vec![format!("the item type {} is or contains a list: a list does not hold lists", describe(item))]);
    }
    let any = SimpleType::builtin("anySimpleType");
    Ok(Arc::new(SimpleType { name, variety: Variety::List(item.clone()), derivation: Derivation::List, base: any, facets: Facets::default() }))
}

/// A union type of `members`, which are tried in order.
pub fn union_of(members: &[Arc<SimpleType>], name: Option<ExpandedName>) -> Result<Arc<SimpleType>, Vec<String>> {
    if members.is_empty() {
        return Err(vec!["a union type needs at least one member type".to_owned()]);
    }
    let any = SimpleType::builtin("anySimpleType");
    Ok(Arc::new(SimpleType { name, variety: Variety::Union(members.to_vec()), derivation: Derivation::Union, base: any, facets: Facets::default() }))
}
