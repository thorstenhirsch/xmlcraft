//! Values that fit a simple type.
//!
//! A value is found by trying candidates in order of preference and keeping the first that the type itself accepts: the enumeration of
//! the type, strings that match its patterns (made from the syntax tree of the pattern), then what suits the kind of type (numbers inside
//! the bounds, a date, a name). Nothing is assumed about what the facets allow: each candidate is checked.

use std::collections::HashMap;

use regex_syntax::hir::{Class, Hir, HirKind};
use xmlcraft_datatypes::{Facets, NoNamespaces, Primitive, SimpleType, Value, Variety, to_base64, to_hex};
use xmlcraft_pattern::Pattern;

/// The longest string made from a pattern.
const MAX_GENERATED: usize = 4096;
/// The most items of a list.
const MAX_ITEMS: usize = 16;
/// How many different strings are made from one pattern.
const PATTERN_VARIANTS: usize = 6;
/// How deep list and union types are followed.
const MAX_NESTING: usize = 8;

/// How the text of elements and attributes is filled in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fill {
    /// A value of the kind the type is: `string`, `0`, `2000-01-01`.
    #[default]
    Typed,
    /// Strings are the name of the element or attribute they are the value of (`<title>title</title>`); the rest is typed.
    Named,
}

#[derive(Debug)]
pub struct Values {
    fill: Fill,
    serial: usize,
    /// How often each plain string was used, so that repeated items differ (`string`, `string2`, …).
    plain: HashMap<String, usize>,
    ids: usize,
    refs: usize,
    /// How many numbers were made, so that each is a different one.
    numbers: usize,
    /// How often a value was made for each name, so that an enumeration is walked through in turn for each of them.
    turns: HashMap<String, usize>,
    turn: usize,
}

/// The facets of a type and of those it is derived from, most derived first.
fn chain(ty: &SimpleType) -> Vec<&Facets> {
    let mut out = Vec::new();
    let mut current = Some(ty);
    while let Some(t) = current {
        out.push(t.facets());
        if out.len() > 256 {
            break;
        }
        current = t.base().map(|b| b.as_ref());
    }
    out
}

fn first<T>(chain: &[&Facets], get: impl Fn(&Facets) -> Option<T>) -> Option<T> {
    chain.iter().find_map(|f| get(f))
}

fn accepts(ty: &SimpleType, candidate: &str) -> bool {
    ty.validate(candidate, &NoNamespaces).is_ok()
}

impl Values {
    pub fn new(fill: Fill) -> Self {
        Self { fill, serial: 0, plain: HashMap::new(), ids: 0, refs: 0, numbers: 0, turns: HashMap::new(), turn: 0 }
    }

    /// A value of `ty` for something called `hint`, as it is written; `None` when no candidate was accepted.
    pub fn make(&mut self, ty: &SimpleType, hint: &str) -> Option<String> {
        self.serial += 1;
        let count = self.turns.entry(hint.to_owned()).or_insert(0);
        self.turn = *count;
        *count += 1;
        self.make_at(ty, hint, 0)
    }

    /// Whether a reference to an ID was made and no ID: the document is not valid then, and cannot be made so by this value alone.
    pub fn dangling_references(&self) -> bool {
        self.refs > 0 && self.ids == 0
    }

    fn make_at(&mut self, ty: &SimpleType, hint: &str, depth: usize) -> Option<String> {
        if depth > MAX_NESTING {
            return None;
        }
        let steps = chain(ty);
        // An enumeration says exactly what is allowed, however the type is built.
        if let Some(values) = steps.iter().find_map(|f| f.enumeration.as_ref()).filter(|v| !v.is_empty()) {
            for offset in 0..values.len() {
                if let Some(value) = values.get((self.turn + offset) % values.len()) {
                    let lexical = value.canonical();
                    if accepts(ty, &lexical) {
                        return Some(lexical);
                    }
                }
            }
        }
        match ty.variety() {
            Variety::List(item) => {
                let wanted = first(&steps, |f| f.length).or_else(|| first(&steps, |f| f.min_length)).unwrap_or(1);
                let count = wanted.min(first(&steps, |f| f.max_length).unwrap_or(usize::MAX)).min(MAX_ITEMS);
                let one = self.make_at(item, hint, depth + 1)?;
                let list = vec![one; count].join(" ");
                accepts(ty, &list).then_some(list)
            }
            Variety::Union(members) => members.iter().find_map(|m| self.make_at(m, hint, depth + 1).filter(|v| accepts(ty, v))),
            Variety::Atomic(primitive) => {
                let candidates = self.candidates(ty, *primitive, hint, &steps);
                candidates.into_iter().find(|c| accepts(ty, c))
            }
        }
    }

    /// A string nobody has been given yet: `base`, then `base2`, `base3`, …
    fn plain(&mut self, base: &str) -> String {
        let count = self.plain.entry(base.to_owned()).or_insert(0);
        *count += 1;
        if *count == 1 { base.to_owned() } else { format!("{base}{count}") }
    }

    fn candidates(&mut self, ty: &SimpleType, primitive: Primitive, hint: &str, steps: &[&Facets]) -> Vec<String> {
        // Strings made from the patterns: the most derived step that has some decides which. The built-in numbers and dates have
        // patterns of their own, which is why these come after what suits the kind of type, which is checked against them anyway.
        let mut from_patterns: Vec<String> = Vec::new();
        for facets in steps {
            for pattern in &facets.patterns {
                for variant in 0..PATTERN_VARIANTS {
                    if let Some(s) = from_pattern(pattern, variant) {
                        from_patterns.push(s);
                    }
                }
            }
            if !from_patterns.is_empty() {
                break;
            }
        }
        let mut out: Vec<String> = Vec::new();
        if ty.is_derived_from_builtin("ID") {
            self.ids += 1;
            out.push(format!("id{}", self.ids));
            return out;
        }
        if ty.is_derived_from_builtin("IDREF") {
            // The first ID of the document is `id1`.
            self.refs += 1;
            out.push("id1".to_owned());
            return out;
        }
        match primitive {
            Primitive::String | Primitive::AnySimple => {
                let name = self.name_value(hint);
                let preferred = self.plain(&name);
                let mut strings = vec![preferred, "string".to_owned(), "a".to_owned(), "en".to_owned(), "name".to_owned()];
                // Lengths: pad or cut what is there.
                let exact = first(steps, |f| f.length);
                let at_least = exact.or_else(|| first(steps, |f| f.min_length)).unwrap_or(0);
                let at_most = exact.or_else(|| first(steps, |f| f.max_length));
                let fitted: Vec<String> = strings.iter().map(|s| fit(s, at_least, at_most)).collect();
                strings.extend(fitted);
                out.extend(strings);
            }
            Primitive::Boolean => out.extend(["true".to_owned(), "false".to_owned()]),
            Primitive::Decimal | Primitive::Float | Primitive::Double => {
                let integer = ty.is_derived_from_builtin("integer");
                self.numbers += 1;
                out.extend(numbers(steps, integer, self.numbers));
            }
            Primitive::Duration => out.extend(["P1D".to_owned(), "PT1H".to_owned(), "P1Y".to_owned()]),
            Primitive::DateTime => out.extend(bounds(steps).into_iter().chain(["2000-01-01T00:00:00".to_owned(), "2000-01-01T12:00:00Z".to_owned()])),
            Primitive::Time => out.extend(bounds(steps).into_iter().chain(["12:00:00".to_owned(), "00:00:00".to_owned()])),
            Primitive::Date => out.extend(bounds(steps).into_iter().chain(["2000-01-01".to_owned(), "2024-06-15".to_owned()])),
            Primitive::GYearMonth => out.extend(bounds(steps).into_iter().chain(["2000-01".to_owned()])),
            Primitive::GYear => out.extend(bounds(steps).into_iter().chain(["2000".to_owned()])),
            Primitive::GMonthDay => out.extend(bounds(steps).into_iter().chain(["--01-01".to_owned()])),
            Primitive::GDay => out.extend(bounds(steps).into_iter().chain(["---01".to_owned()])),
            Primitive::GMonth => out.extend(bounds(steps).into_iter().chain(["--01".to_owned()])),
            Primitive::HexBinary => {
                let n = first(steps, |f| f.length).or_else(|| first(steps, |f| f.min_length)).unwrap_or(1).min(MAX_GENERATED / 2);
                out.push(to_hex(&vec![0u8; n]));
            }
            Primitive::Base64Binary => {
                let n = first(steps, |f| f.length).or_else(|| first(steps, |f| f.min_length)).unwrap_or(1).min(MAX_GENERATED / 2);
                out.push(to_base64(&vec![0u8; n]));
            }
            Primitive::AnyUri => out.extend(["http://example.com/".to_owned(), "a".to_owned()]),
            Primitive::QName => out.extend(["a".to_owned()]),
            Primitive::Notation => {}
        }
        out.extend(from_patterns);
        out
    }

    /// The text a string value has when it is filled in with names: the hint made into a name.
    fn name_value(&self, hint: &str) -> String {
        if self.fill == Fill::Typed {
            return "string".to_owned();
        }
        let clean: String = hint.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { 'x' }).collect();
        if clean.is_empty() { "string".to_owned() } else { clean }
    }
}

/// `text` made at least `least` and at most `most` characters long (padded with `a`, or cut).
fn fit(text: &str, least: usize, most: Option<usize>) -> String {
    let mut s: String = text.to_owned();
    while s.chars().count() < least && s.len() < MAX_GENERATED {
        s.push('a');
    }
    if let Some(most) = most {
        s = s.chars().take(most).collect();
    }
    s
}

/// The bounds a type puts on its values, as they are written (the ones that can be values themselves first).
fn bounds(steps: &[&Facets]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(v) = first(steps, |f| f.min_inclusive.as_ref().map(Value::canonical)) {
        out.push(v);
    }
    if let Some(v) = first(steps, |f| f.max_inclusive.as_ref().map(Value::canonical)) {
        out.push(v);
    }
    out
}

fn parse_number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

/// Candidates for a number: near the bounds when there are some, else small ones; a different one for each call (`serial`).
fn numbers(steps: &[&Facets], integer: bool, serial: usize) -> Vec<String> {
    let lo_inclusive = first(steps, |f| f.min_inclusive.as_ref().map(Value::canonical));
    let lo_exclusive = first(steps, |f| f.min_exclusive.as_ref().map(Value::canonical));
    let hi_inclusive = first(steps, |f| f.max_inclusive.as_ref().map(Value::canonical));
    let hi_exclusive = first(steps, |f| f.max_exclusive.as_ref().map(Value::canonical));
    let lo = lo_inclusive.as_deref().and_then(parse_number).or_else(|| lo_exclusive.as_deref().and_then(parse_number).map(|n| n + 1.0));
    let hi = hi_inclusive.as_deref().and_then(parse_number).or_else(|| hi_exclusive.as_deref().and_then(parse_number).map(|n| n - 1.0));
    let show = |n: f64| -> String { if integer || n.fract() == 0.0 { format!("{}", n as i64) } else { format!("{n}") } };
    let mut out = Vec::new();
    let serial = serial as f64;
    // A small number, a different one each time, moved in to the bounds when it is outside them.
    let mut near = serial;
    if let Some(lo) = lo
        && near < lo
    {
        near = lo + serial - 1.0;
    }
    if let Some(hi) = hi
        && near > hi
    {
        near = hi - serial + 1.0;
    }
    out.push(show(near));
    if let Some(lo) = lo {
        out.push(show(lo));
    }
    if let Some(hi) = hi {
        out.push(show(hi));
    }
    if let (Some(lo), Some(hi)) = (lo, hi) {
        out.push(show(((lo + hi) / 2.0).floor()));
    }
    out.extend(lo_inclusive);
    out.extend(hi_inclusive);
    out.extend(["0".to_owned(), "1".to_owned(), "-1".to_owned(), "100".to_owned()]);
    if !integer {
        out.extend(["1.5".to_owned(), "0.5".to_owned()]);
    }
    out
}

/// A string that matches `pattern`; `variant` chooses between the ways it can (the branch of an alternative, how many repetitions).
fn from_pattern(pattern: &Pattern, variant: usize) -> Option<String> {
    let mut out = String::new();
    generate(pattern.hir(), variant, &mut out).then_some(out).filter(|s| pattern.is_match(s))
}

/// The characters tried first when a class has to give one.
const PREFERRED: [char; 12] = ['a', 'b', 'x', 'A', 'B', '0', '1', '2', '-', '_', '.', ' '];

fn generate(hir: &Hir, variant: usize, out: &mut String) -> bool {
    if out.len() > MAX_GENERATED {
        return false;
    }
    match hir.kind() {
        HirKind::Empty | HirKind::Look(_) => true,
        HirKind::Literal(literal) => {
            out.push_str(&String::from_utf8_lossy(&literal.0));
            true
        }
        HirKind::Class(Class::Unicode(class)) => {
            let ranges = class.ranges();
            let contains = |c: char| ranges.iter().any(|r| r.start() <= c && c <= r.end());
            let matching: Vec<char> = PREFERRED.iter().copied().filter(|c| contains(*c)).collect();
            let chosen = if matching.is_empty() { ranges.first().map(|r| r.start()) } else { matching.get(variant % matching.len()).copied() };
            match chosen {
                Some(c) => {
                    out.push(c);
                    true
                }
                None => false,
            }
        }
        HirKind::Class(Class::Bytes(class)) => match class.ranges().first() {
            Some(r) => {
                out.push(char::from(r.start()));
                true
            }
            None => false,
        },
        HirKind::Repetition(repetition) => {
            let min = usize::try_from(repetition.min).unwrap_or(usize::MAX);
            let wanted = min.saturating_add(variant % 3);
            let count = match repetition.max {
                Some(max) => wanted.min(usize::try_from(max).unwrap_or(usize::MAX)),
                None => wanted,
            };
            if count > MAX_GENERATED {
                return false;
            }
            (0..count).all(|_| generate(&repetition.sub, variant, out))
        }
        HirKind::Capture(capture) => generate(&capture.sub, variant, out),
        HirKind::Concat(parts) => parts.iter().all(|p| generate(p, variant, out)),
        HirKind::Alternation(branches) => match branches.get(variant % branches.len().max(1)) {
            Some(branch) => generate(branch, variant / branches.len().max(1), out),
            None => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use xmlcraft_datatypes::{FacetSpec, Setting, list_of, restrict};

    use super::*;

    fn builtin(name: &str) -> Arc<SimpleType> {
        SimpleType::builtin(name).expect("a built-in type")
    }

    fn made(ty: &SimpleType, hint: &str) -> String {
        Values::new(Fill::Typed).make(ty, hint).expect("a value")
    }

    #[test]
    fn the_built_in_types_get_values_of_their_kind() {
        for name in [
            "string",
            "boolean",
            "decimal",
            "float",
            "double",
            "duration",
            "dateTime",
            "time",
            "date",
            "gYearMonth",
            "gYear",
            "gMonthDay",
            "gDay",
            "gMonth",
            "hexBinary",
            "base64Binary",
            "anyURI",
            "QName",
            "integer",
            "int",
            "long",
            "short",
            "byte",
            "nonNegativeInteger",
            "positiveInteger",
            "negativeInteger",
            "nonPositiveInteger",
            "unsignedByte",
            "unsignedLong",
            "token",
            "normalizedString",
            "language",
            "Name",
            "NCName",
            "NMTOKEN",
            "NMTOKENS",
            "ID",
            "IDREF",
            "IDREFS",
            "ENTITY",
        ] {
            let ty = builtin(name);
            let value = Values::new(Fill::Typed).make(&ty, "x");
            assert!(value.as_deref().is_some_and(|v| accepts(&ty, v)), "{name}: {value:?}");
        }
    }

    #[test]
    fn named_fill_uses_the_name_for_strings_only() {
        let mut v = Values::new(Fill::Named);
        assert_eq!(v.make(&builtin("string"), "title").as_deref(), Some("title"));
        assert_eq!(v.make(&builtin("int"), "title").as_deref(), Some("1"), "numbers are numbers");
        assert_eq!(Values::new(Fill::Typed).make(&builtin("string"), "title").as_deref(), Some("string"));
    }

    #[test]
    fn repeated_strings_differ() {
        let mut v = Values::new(Fill::Typed);
        let ty = builtin("string");
        let all: Vec<String> = (0..3).filter_map(|_| v.make(&ty, "t")).collect();
        assert_eq!(all, ["string", "string2", "string3"]);
        let mut n = Values::new(Fill::Typed);
        let ints: Vec<String> = (0..3).filter_map(|_| n.make(&builtin("int"), "n")).collect();
        assert_eq!(ints, ["1", "2", "3"]);
    }

    fn restricted(base: &str, spec: FacetSpec) -> Arc<SimpleType> {
        restrict(&builtin(base), None, &spec, &NoNamespaces).expect("a restriction")
    }

    fn setting(value: &str) -> Option<Setting> {
        Some(Setting::new(value))
    }

    #[test]
    fn facets_are_honoured() {
        let enumeration = restricted("string", FacetSpec { enumeration: vec!["red".into(), "green".into()], ..FacetSpec::default() });
        let mut v = Values::new(Fill::Typed);
        let got: Vec<String> = (0..3).filter_map(|_| v.make(&enumeration, "c")).collect();
        assert!(got.iter().all(|s| s == "red" || s == "green") && got.contains(&"red".to_owned()) && got.contains(&"green".to_owned()), "{got:?}");
        let ranged = restricted("integer", FacetSpec { min_inclusive: setting("10"), max_inclusive: setting("12"), ..FacetSpec::default() });
        let n: i64 = made(&ranged, "n").parse().expect("a number");
        assert!((10..=12).contains(&n), "{n}");
        let exclusive = restricted("int", FacetSpec { min_exclusive: setting("100"), ..FacetSpec::default() });
        assert!(made(&exclusive, "n").parse::<i64>().expect("a number") > 100);
        let long = restricted("string", FacetSpec { length: setting("5"), ..FacetSpec::default() });
        assert_eq!(made(&long, "s").chars().count(), 5);
        let bounded = restricted("string", FacetSpec { min_length: setting("3"), max_length: setting("4"), ..FacetSpec::default() });
        assert!((3..=4).contains(&made(&bounded, "s").chars().count()));
    }

    #[test]
    fn patterns_are_matched() {
        for (pattern, ty) in [
            ("[A-Z]{2}[0-9]{3}", "string"),
            ("(red|green|blue)", "string"),
            ("\\d{3}-\\d{4}", "string"),
            ("[a-z]+@[a-z]+\\.com", "string"),
            ("[0-9]+", "integer"),
        ] {
            let t = restricted(ty, FacetSpec { patterns: vec![pattern.into()], ..FacetSpec::default() });
            let value = Values::new(Fill::Typed).make(&t, "p");
            assert!(value.as_deref().is_some_and(|v| accepts(&t, v)), "{pattern}: {value:?}");
        }
    }

    #[test]
    fn lists_and_binary_values_have_the_length_the_facets_ask_for() {
        let list = list_of(&builtin("int"), None).expect("a list");
        let three = restrict(&list, None, &FacetSpec { length: setting("3"), ..FacetSpec::default() }, &NoNamespaces).expect("a restriction");
        assert_eq!(made(&three, "l").split(' ').count(), 3);
        let two_to_four =
            restrict(&list, None, &FacetSpec { min_length: setting("2"), max_length: setting("4"), ..FacetSpec::default() }, &NoNamespaces)
                .expect("a restriction");
        assert!((2..=4).contains(&made(&two_to_four, "l").split(' ').count()));
        assert_eq!(made(&list, "l").split(' ').count(), 1, "one item when nothing asks for more");
        let hex = restricted("hexBinary", FacetSpec { length: setting("3"), ..FacetSpec::default() });
        assert_eq!(made(&hex, "h").len(), 6, "three octets");
        let base64 = restricted("base64Binary", FacetSpec { length: setting("5"), ..FacetSpec::default() });
        assert!(accepts(&base64, &made(&base64, "b")));
    }

    #[test]
    fn a_value_nothing_can_make_is_none() {
        let impossible = restricted("string", FacetSpec { patterns: vec!["[a-z]{3}".into()], max_length: setting("1"), ..FacetSpec::default() });
        assert_eq!(Values::new(Fill::Typed).make(&impossible, "x"), None);
    }

    #[test]
    fn ids_are_numbered_and_references_point_at_the_first() {
        let mut v = Values::new(Fill::Typed);
        assert_eq!(v.make(&builtin("ID"), "a").as_deref(), Some("id1"));
        assert_eq!(v.make(&builtin("ID"), "a").as_deref(), Some("id2"));
        assert_eq!(v.make(&builtin("IDREF"), "r").as_deref(), Some("id1"));
        assert!(!v.dangling_references());
        assert!(Values::new(Fill::Typed).make(&builtin("IDREF"), "r").is_some());
    }
}
