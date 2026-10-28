//! Properties of the lexical mappings and the orders: nothing panics, canonical forms read back as the same value, the orders agree
//! with an independent computation.

use std::cmp::Ordering;

use chrono::{FixedOffset, TimeZone};
use proptest::prelude::*;
use xmlcraft_datatypes::{NoNamespaces, SimpleType, Value, parse_base64, parse_hex, to_base64, to_hex};

fn value(name: &str, lexical: &str) -> Option<Value> {
    SimpleType::builtin(name)?.validate(lexical, &NoNamespaces).ok()
}

proptest! {
    #[test]
    fn no_text_makes_a_built_in_type_panic(text in "[0-9a-zA-Z+\\-.:TZPEe/ \\t\\n=_#%]{0,40}", other in "\\PC{0,30}") {
        for name in SimpleType::builtin_names() {
            if let Some(t) = SimpleType::builtin(name) {
                for lexical in [&text, &other] {
                    if let Ok(v) = t.validate(lexical, &NoNamespaces) {
                        let canonical = v.canonical();
                        // Whatever is valid has a canonical form, and the value equals itself (unless it is NaN, which equals itself too).
                        prop_assert!(v.equals(&v), "{} {:?}", name, lexical);
                        let _ = canonical;
                    }
                }
            }
        }
    }

    #[test]
    fn decimals_are_ordered_like_the_numbers_they_name(
        a_neg in any::<bool>(), a_int in "[0-9]{0,15}", a_frac in "[0-9]{0,6}",
        b_neg in any::<bool>(), b_int in "[0-9]{0,15}", b_frac in "[0-9]{0,6}",
    ) {
        prop_assume!(!(a_int.is_empty() && a_frac.is_empty()) && !(b_int.is_empty() && b_frac.is_empty()));
        let render = |neg: bool, int: &str, frac: &str| format!("{}{}{}{}", if neg { "-" } else { "" }, int, if frac.is_empty() { "" } else { "." }, frac);
        let scaled = |neg: bool, int: &str, frac: &str| -> i128 {
            let whole: i128 = if int.is_empty() { 0 } else { int.parse().expect("digits") };
            let fraction: i128 = format!("{frac:0<6}").parse().expect("digits");
            let magnitude = whole * 1_000_000 + fraction;
            if neg { -magnitude } else { magnitude }
        };
        let (a, b) = (render(a_neg, &a_int, &a_frac), render(b_neg, &b_int, &b_frac));
        let (va, vb) = (value("decimal", &a).expect("a decimal"), value("decimal", &b).expect("a decimal"));
        let expected = scaled(a_neg, &a_int, &a_frac).cmp(&scaled(b_neg, &b_int, &b_frac));
        prop_assert_eq!(va.compare(&vb), Some(expected), "{} {}", a, b);
        prop_assert_eq!(va.equals(&vb), expected == Ordering::Equal);
    }

    #[test]
    fn canonical_forms_read_back_as_the_same_value(
        bytes in prop::collection::vec(any::<u8>(), 0..40),
        mantissa in -999_999_999i64..999_999_999, exponent in -30i32..30,
        days in 0u32..400, hours in 0u32..30, minutes in 0u32..90, seconds in 0u32..90, tenths in 0u32..10, months in 0u32..30, negative in any::<bool>(),
    ) {
        let hex = to_hex(&bytes);
        prop_assert_eq!(parse_hex(&hex), Some(bytes.clone()));
        prop_assert_eq!(parse_hex(&hex.to_lowercase()), Some(bytes.clone()));
        let b64 = to_base64(&bytes);
        prop_assert_eq!(parse_base64(&b64), Some(bytes.clone()));
        let double = format!("{mantissa}e{exponent}");
        for name in ["double", "float", "decimal"] {
            let lexical = if name == "decimal" { mantissa.to_string() } else { double.clone() };
            let v = value(name, &lexical).expect("a number");
            let canonical = v.canonical();
            let again = value(name, &canonical).expect("the canonical form is a number");
            prop_assert!(v.equals(&again), "{} {} -> {}", name, lexical, canonical);
            prop_assert_eq!(again.canonical(), canonical);
        }
        let duration = format!("{}P{months}M{days}DT{hours}H{minutes}M{seconds}.{tenths}S", if negative { "-" } else { "" });
        let v = value("duration", &duration).expect("a duration");
        let canonical = v.canonical();
        let again = value("duration", &canonical).expect("the canonical form is a duration");
        prop_assert!(v.equals(&again), "{} -> {}", duration, canonical);
        prop_assert_eq!(again.canonical(), canonical);
    }

    #[test]
    fn date_times_are_ordered_like_instants(
        a in (1i32..9999, 1u32..=12, 1u32..=28, 0u32..24, 0u32..60, 0u32..60, -840i32..=840),
        b in (1i32..9999, 1u32..=12, 1u32..=28, 0u32..24, 0u32..60, 0u32..60, -840i32..=840),
    ) {
        let lexical = |(y, mo, d, h, mi, s, off): (i32, u32, u32, u32, u32, u32, i32)| {
            format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}{}{:02}:{:02}", if off < 0 { '-' } else { '+' }, off.abs() / 60, off.abs() % 60)
        };
        let instant = |(y, mo, d, h, mi, s, off): (i32, u32, u32, u32, u32, u32, i32)| {
            FixedOffset::east_opt(off * 60).and_then(|tz| tz.with_ymd_and_hms(y, mo, d, h, mi, s).single()).expect("a date")
        };
        let (la, lb) = (lexical(a), lexical(b));
        let (va, vb) = (value("dateTime", &la).expect("a dateTime"), value("dateTime", &lb).expect("a dateTime"));
        let expected = instant(a).cmp(&instant(b));
        prop_assert_eq!(va.compare(&vb), Some(expected), "{} {}", la, lb);
        prop_assert_eq!(vb.compare(&va), Some(expected.reverse()));
        prop_assert_eq!(va.equals(&vb), expected == Ordering::Equal);
    }

    #[test]
    fn dates_with_and_without_a_zone_are_never_ordered_wrongly(
        y in 1i32..9999, mo in 1u32..=12, d in 1u32..=28, h in 0u32..24, off in -840i32..=840,
        local_days in -3i64..=3, local_hours in 0u32..24,
    ) {
        // A value with a zone, and one without whose day is `local_days` away: when they are ordered, the order is the one
        // every possible zone of the second gives; otherwise some zone gives each of the orders (or they are equal).
        let with = format!("{y:04}-{mo:02}-{d:02}T{h:02}:00:00{}{:02}:{:02}", if off < 0 { '-' } else { '+' }, off.abs() / 60, off.abs() % 60);
        let base = chrono::NaiveDate::from_ymd_opt(y, mo, d).expect("a date") + chrono::Days::new(5);
        let shifted = base.checked_add_signed(chrono::TimeDelta::days(local_days)).expect("a date");
        let without = format!("{}T{local_hours:02}:00:00", shifted.format("%Y-%m-%d"));
        let (a, b) = (value("dateTime", &with).expect("a dateTime"), value("dateTime", &without).expect("a dateTime"));
        let order = a.compare(&b);
        let first = FixedOffset::east_opt(off * 60).and_then(|tz| tz.with_ymd_and_hms(y, mo, d, h, 0, 0).single()).expect("a date").naive_utc();
        let second = shifted.and_hms_opt(local_hours, 0, 0).expect("a time");
        let mut seen = std::collections::BTreeSet::new();
        for zone in [-840, 840] {
            let zoned = second - chrono::TimeDelta::minutes(i64::from(zone));
            seen.insert(first.cmp(&zoned));
        }
        match order {
            Some(Ordering::Less) => prop_assert!(seen.iter().all(|o| *o == Ordering::Less), "{} {}", with, without),
            Some(Ordering::Greater) => prop_assert!(seen.iter().all(|o| *o == Ordering::Greater), "{} {}", with, without),
            Some(Ordering::Equal) => prop_assert!(false, "a value with a zone is never equal to one without"),
            None => prop_assert!(seen.len() > 1 || seen.contains(&Ordering::Equal), "{} {}: indeterminate but every zone agrees", with, without),
        }
    }

    #[test]
    fn durations_without_months_are_ordered_like_their_seconds_and_without_seconds_like_their_months(
        a in (0u32..50, 0u32..100, 0u32..200, 0u32..200), b in (0u32..50, 0u32..100, 0u32..200, 0u32..200), ma in 0u32..500, mb in 0u32..500,
    ) {
        let lexical = |(d, h, m, s): (u32, u32, u32, u32)| format!("P{d}DT{h}H{m}M{s}S");
        let total = |(d, h, m, s): (u32, u32, u32, u32)| u64::from(d) * 86_400 + u64::from(h) * 3600 + u64::from(m) * 60 + u64::from(s);
        let (va, vb) = (value("duration", &lexical(a)).expect("a duration"), value("duration", &lexical(b)).expect("a duration"));
        prop_assert_eq!(va.compare(&vb), Some(total(a).cmp(&total(b))));
        let (xa, xb) = (value("duration", &format!("P{ma}M")).expect("a duration"), value("duration", &format!("P{mb}M")).expect("a duration"));
        prop_assert_eq!(xa.compare(&xb), Some(ma.cmp(&mb)));
        // And the order is antisymmetric whatever is mixed.
        let mixed = value("duration", &format!("P{ma}MT{}S", total(a) % 1000)).expect("a duration");
        let other = value("duration", &format!("P{mb}MT{}S", total(b) % 1000)).expect("a duration");
        prop_assert_eq!(mixed.compare(&other), other.compare(&mixed).map(Ordering::reverse));
    }

    #[test]
    fn a_list_is_its_items_whatever_separates_them(items in prop::collection::vec(-1000i32..1000, 0..8), seps in prop::collection::vec(prop::sample::select(vec![" ", "  ", "\t", "\n", " \r\n "]), 8)) {
        let mut text = String::new();
        for (i, item) in items.iter().enumerate() {
            text.push_str(&item.to_string());
            text.push_str(seps.get(i).copied().unwrap_or(" "));
        }
        let list = xmlcraft_datatypes::list_of(&SimpleType::builtin("integer").expect("integer"), None).expect("a list");
        let v = list.validate(&text, &NoNamespaces).expect("a list of integers");
        prop_assert_eq!(v.length(), Some(items.len()));
        let expected = items.iter().map(i32::to_string).collect::<Vec<_>>().join(" ");
        prop_assert_eq!(v.canonical(), expected);
    }
}
