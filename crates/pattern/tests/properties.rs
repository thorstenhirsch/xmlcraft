//! Properties: any text is a pattern or an error and never a panic, and a pattern matches the values that an independent reading of the
//! same expression says it does.

use std::collections::BTreeSet;

use proptest::prelude::*;
use xmlcraft_pattern::Pattern;

/// An expression, as the tests think of it (not the crate's own tree).
#[derive(Clone, Debug)]
enum T {
    Lit(char),
    Range(char, char),
    NotIn(Vec<char>),
    Dot,
    Cat(Vec<T>),
    Alt(Vec<T>),
    Rep(Box<T>, u32, Option<u32>),
}

fn letter() -> impl Strategy<Value = char> {
    prop::sample::select(vec!['a', 'b', 'c'])
}

fn tree() -> impl Strategy<Value = T> {
    let leaf = prop_oneof![
        letter().prop_map(T::Lit),
        (letter(), letter()).prop_map(|(x, y)| T::Range(x.min(y), x.max(y))),
        prop::collection::vec(letter(), 1..3).prop_map(T::NotIn),
        Just(T::Dot),
    ];
    leaf.prop_recursive(4, 24, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 1..4).prop_map(T::Cat),
            prop::collection::vec(inner.clone(), 2..4).prop_map(T::Alt),
            (inner, 0..3u32, prop::option::of(0..3u32)).prop_map(|(t, min, max)| T::Rep(Box::new(t), min, max.map(|m| m.max(min)))),
        ]
    })
}

/// The expression as a pattern.
fn render(t: &T) -> String {
    match t {
        T::Lit(c) => c.to_string(),
        T::Range(lo, hi) => format!("[{lo}-{hi}]"),
        T::NotIn(v) => format!("[^{}]", v.iter().collect::<String>()),
        T::Dot => ".".to_owned(),
        T::Cat(v) => v.iter().map(render).collect(),
        T::Alt(v) => format!("({})", v.iter().map(render).collect::<Vec<_>>().join("|")),
        T::Rep(t, min, max) => {
            let quantifier = match max {
                Some(max) => format!("{{{min},{max}}}"),
                None => format!("{{{min},}}"),
            };
            format!("({}){quantifier}", render(t))
        }
    }
}

/// Every position where a match of `t` that starts at `from` can end.
fn ends(t: &T, s: &[char], from: usize) -> BTreeSet<usize> {
    let one = |ok: &dyn Fn(char) -> bool| s.get(from).filter(|c| ok(**c)).map(|_| BTreeSet::from([from + 1])).unwrap_or_default();
    match t {
        T::Lit(c) => one(&|x| x == *c),
        T::Range(lo, hi) => one(&|x| (*lo..=*hi).contains(&x)),
        T::NotIn(v) => one(&|x| !v.contains(&x)),
        T::Dot => one(&|x| x != '\n' && x != '\r'),
        T::Cat(v) => v.iter().fold(BTreeSet::from([from]), |at, t| at.iter().flat_map(|p| ends(t, s, *p)).collect()),
        T::Alt(v) => v.iter().flat_map(|t| ends(t, s, from)).collect(),
        T::Rep(t, min, max) => {
            let cap = max.unwrap_or(s.len() as u32 + *min + 1);
            let mut at = BTreeSet::from([from]);
            let mut out = BTreeSet::new();
            if *min == 0 {
                out.insert(from);
            }
            for k in 1..=cap {
                at = at.iter().flat_map(|p| ends(t, s, *p)).collect();
                if k >= *min {
                    out.extend(at.iter().copied());
                }
                if at.is_empty() {
                    break;
                }
            }
            out
        }
    }
}

fn value() -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(vec!['a', 'b', 'c', 'd', '\n']), 0..7).prop_map(|v| v.into_iter().collect())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn a_pattern_matches_what_an_independent_reading_of_it_matches(t in tree(), values in prop::collection::vec(value(), 1..8)) {
        let source = render(&t);
        let pattern = Pattern::new(&source).map_err(|e| TestCaseError::fail(format!("{source}: {e}")))?;
        for v in values {
            let chars: Vec<char> = v.chars().collect();
            let expected = ends(&t, &chars, 0).contains(&chars.len());
            prop_assert_eq!(pattern.is_match(&v), expected, "{} on {:?}", source, v);
        }
    }

    #[test]
    fn no_text_makes_a_pattern_panic(source in "[()\\[\\]{}|*+?\\\\^.a-c0-9,pPLuIs\\-]{0,24}", value in "\\PC{0,12}") {
        if let Ok(pattern) = Pattern::new(&source) {
            let _ = pattern.is_match(&value);
            let _ = pattern.hir();
        }
    }

    #[test]
    fn any_text_is_read_without_a_panic(source in "\\PC{0,60}") {
        let _ = Pattern::new(&source);
    }

    #[test]
    fn an_error_points_into_the_pattern(source in "[()\\[\\]{}|*+?\\\\^.a-c0-9,pPLuIs\\-]{0,24}") {
        if let Err(e) = Pattern::new(&source) {
            prop_assert!(e.position <= source.chars().count(), "{} at {}", source, e.position);
            prop_assert!(!e.message().is_empty());
        }
    }
}
