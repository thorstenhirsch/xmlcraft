//! The values seen for an attribute, or for the text of an element, and the simple type and enumeration they suggest.

use std::collections::BTreeMap;
use std::sync::Arc;

use xmlcraft_datatypes::{NoNamespaces, SimpleType, is_nmtoken};

/// The most distinct values kept for one attribute or text; past it the values are only counted.
pub const MAX_DISTINCT: usize = 512;
/// A value longer than this (in characters) is not a candidate for an enumeration.
pub const MAX_ENUMERATED_LEN: usize = 128;

/// The simple types a set of values can be inferred as, the narrowest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Boolean,
    Integer,
    Decimal,
    Double,
    Date,
    DateTime,
    Time,
    String,
}

impl Kind {
    /// The name of the built-in type of XML Schema.
    pub fn xsd_name(self) -> &'static str {
        match self {
            Kind::Boolean => "boolean",
            Kind::Integer => "integer",
            Kind::Decimal => "decimal",
            Kind::Double => "double",
            Kind::Date => "date",
            Kind::DateTime => "dateTime",
            Kind::Time => "time",
            Kind::String => "string",
        }
    }

    const NARROW: [Kind; 7] = [Kind::Boolean, Kind::Integer, Kind::Decimal, Kind::Double, Kind::Date, Kind::DateTime, Kind::Time];

    fn bit(self) -> u8 {
        match self {
            Kind::Boolean => 1,
            Kind::Integer => 2,
            Kind::Decimal => 4,
            Kind::Double => 8,
            Kind::Date => 16,
            Kind::DateTime => 32,
            Kind::Time => 64,
            Kind::String => 0,
        }
    }

    /// Whether `value` is a lexical form of this type (a boolean is `true` or `false`: 0 and 1 are numbers).
    fn accepts(self, value: &str, types: &Types) -> bool {
        if self == Kind::Boolean {
            return matches!(value.trim(), "true" | "false");
        }
        types.get(self).is_some_and(|t| t.validate(value, &NoNamespaces).is_ok())
    }
}

/// The built-in types the kinds are tested with.
pub struct Types(BTreeMap<Kind, Arc<SimpleType>>);

impl Types {
    pub fn new() -> Self {
        let mut map = BTreeMap::new();
        for kind in Kind::NARROW {
            if kind != Kind::Boolean
                && let Some(t) = SimpleType::builtin(kind.xsd_name())
            {
                map.insert(kind, t);
            }
        }
        Types(map)
    }

    fn get(&self, kind: Kind) -> Option<&Arc<SimpleType>> {
        self.0.get(&kind)
    }
}

impl std::fmt::Debug for Types {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

impl Default for Types {
    fn default() -> Self {
        Self::new()
    }
}

/// What makes a set of values an enumeration: at most `max_values` different ones, each seen at least `min_occurrences` times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnumerationRule {
    pub max_values: usize,
    pub min_occurrences: usize,
}

impl Default for EnumerationRule {
    fn default() -> Self {
        Self { max_values: 10, min_occurrences: 2 }
    }
}

/// The values seen, with how often.
#[derive(Clone, Debug)]
pub struct ValueSet {
    counts: BTreeMap<String, usize>,
    /// Values past [`MAX_DISTINCT`] were not kept.
    overflow: bool,
    total: usize,
    /// The narrow kinds every value so far is a lexical form of (bits of [`Kind::bit`]).
    candidates: u8,
    long: bool,
}

impl Default for ValueSet {
    fn default() -> Self {
        Self { counts: BTreeMap::new(), overflow: false, total: 0, candidates: Kind::NARROW.iter().fold(0, |bits, k| bits | k.bit()), long: false }
    }
}

impl ValueSet {
    pub fn observe(&mut self, value: &str, types: &Types) {
        self.total = self.total.saturating_add(1);
        if let Some(count) = self.counts.get_mut(value) {
            *count = count.saturating_add(1);
            return;
        }
        // A value seen before cannot change what the values can be; a new one may.
        if self.candidates != 0 {
            for kind in Kind::NARROW {
                if self.candidates & kind.bit() != 0 && !kind.accepts(value, types) {
                    self.candidates &= !kind.bit();
                }
            }
        }
        if value.chars().count() > MAX_ENUMERATED_LEN {
            self.long = true;
        }
        if self.counts.len() < MAX_DISTINCT {
            self.counts.insert(value.to_owned(), 1);
        } else {
            self.overflow = true;
        }
    }

    #[cfg(test)]
    pub fn total(&self) -> usize {
        self.total
    }

    /// Whether some value is not empty.
    pub fn has_non_empty(&self) -> bool {
        self.overflow || self.counts.keys().any(|v| !v.is_empty())
    }

    /// The narrowest type all the values are lexical forms of (a string when there are none).
    pub fn kind(&self) -> Kind {
        if self.total == 0 {
            return Kind::String;
        }
        Kind::NARROW.into_iter().find(|k| self.candidates & k.bit() != 0).unwrap_or(Kind::String)
    }

    /// The values, when they make an enumeration under `rule`: more than one, few, each seen often, none of them long.
    pub fn enumeration(&self, rule: EnumerationRule) -> Option<Vec<String>> {
        let distinct = self.counts.len();
        let fits = !self.overflow
            && !self.long
            && distinct >= 2
            && distinct <= rule.max_values
            && self.counts.values().all(|&n| n >= rule.min_occurrences.max(1))
            && self.kind() != Kind::Boolean;
        fits.then(|| self.counts.keys().cloned().collect())
    }

    /// Whether every value is a name token (what a DTD enumeration needs).
    pub fn all_name_tokens(&self, values: &[String]) -> bool {
        values.iter().all(|v| is_nmtoken(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(values: &[&str]) -> ValueSet {
        let types = Types::new();
        let mut s = ValueSet::default();
        for v in values {
            s.observe(v, &types);
        }
        s
    }

    #[test]
    fn the_narrowest_type_that_every_value_is_a_form_of() {
        assert_eq!(set(&["1", "20", "-3"]).kind(), Kind::Integer);
        assert_eq!(set(&["1", "2.5"]).kind(), Kind::Decimal);
        assert_eq!(set(&["1", "2.5e3"]).kind(), Kind::Double);
        assert_eq!(set(&["true", "false"]).kind(), Kind::Boolean);
        assert_eq!(set(&["0", "1"]).kind(), Kind::Integer, "0 and 1 are numbers, not booleans");
        assert_eq!(set(&["2024-01-31", "2023-12-01"]).kind(), Kind::Date);
        assert_eq!(set(&["2024-01-31T10:00:00"]).kind(), Kind::DateTime);
        assert_eq!(set(&["10:00:00"]).kind(), Kind::Time);
        assert_eq!(set(&["1", "x"]).kind(), Kind::String);
        assert_eq!(set(&["1", ""]).kind(), Kind::String, "an empty value is no number");
        assert_eq!(set(&[]).kind(), Kind::String);
        assert_eq!(set(&["2024-13-45"]).kind(), Kind::String, "a date that is not one");
    }

    #[test]
    fn white_space_around_a_number_does_not_make_it_a_string() {
        assert_eq!(set(&[" 7 ", "8"]).kind(), Kind::Integer);
    }

    #[test]
    fn an_enumeration_is_few_values_each_seen_often() {
        let rule = EnumerationRule { max_values: 3, min_occurrences: 2 };
        assert_eq!(set(&["a", "b", "a", "b"]).enumeration(rule), Some(vec!["a".to_owned(), "b".to_owned()]));
        assert_eq!(set(&["a", "b", "a"]).enumeration(rule), None, "b is seen once");
        assert_eq!(set(&["a", "a", "a"]).enumeration(rule), None, "one value is a constant, not a choice");
        assert_eq!(set(&["a", "b", "c", "d", "a", "b", "c", "d"]).enumeration(rule), None, "too many");
        assert_eq!(set(&["true", "false", "true", "false"]).enumeration(rule), None, "a boolean is already a choice");
        let long = "x".repeat(MAX_ENUMERATED_LEN + 1);
        assert_eq!(set(&[&long, "a", &long, "a"]).enumeration(rule), None, "a long value is no enumerator");
    }

    #[test]
    fn values_past_the_cap_are_counted_and_still_decide_the_type() {
        let types = Types::new();
        let mut s = ValueSet::default();
        for i in 0..(MAX_DISTINCT + 10) {
            s.observe(&i.to_string(), &types);
        }
        assert_eq!(s.kind(), Kind::Integer);
        assert_eq!(s.enumeration(EnumerationRule { max_values: 1000, min_occurrences: 1 }), None, "values were not kept");
        s.observe("not a number", &types);
        assert_eq!(s.kind(), Kind::String, "a value that was not kept still counts for the type");
        assert_eq!(s.total(), MAX_DISTINCT + 11);
    }

    #[test]
    fn name_tokens_are_what_a_dtd_enumeration_can_hold() {
        let s = set(&[]);
        assert!(s.all_name_tokens(&["a".into(), "b-1".into()]));
        assert!(!s.all_name_tokens(&["a b".into()]));
        assert!(!s.all_name_tokens(&["".into()]));
    }
}
