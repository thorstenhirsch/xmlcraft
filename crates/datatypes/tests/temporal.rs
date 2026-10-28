//! Dates, times and durations: what is valid, what is equal, and what comes before what.

use std::cmp::Ordering;

use xmlcraft_datatypes::{NoNamespaces, SimpleType, Value};

fn value(name: &str, lexical: &str) -> Value {
    SimpleType::builtin(name)
        .unwrap_or_else(|| panic!("no built-in type {name}"))
        .validate(lexical, &NoNamespaces)
        .unwrap_or_else(|e| panic!("{lexical:?} should be a {name}: {e}"))
}

fn valid(name: &str, lexical: &str) -> bool {
    SimpleType::builtin(name).is_some_and(|t| t.validate(lexical, &NoNamespaces).is_ok())
}

fn check(name: &str, yes: &[&str], no: &[&str]) {
    for lexical in yes {
        assert!(valid(name, lexical), "{lexical:?} should be a {name}");
    }
    for lexical in no {
        assert!(!valid(name, lexical), "{lexical:?} should not be a {name}");
    }
}

fn order(name: &str, a: &str, b: &str) -> Option<Ordering> {
    value(name, a).compare(&value(name, b))
}

fn same(name: &str, a: &str, b: &str) -> bool {
    value(name, a).equals(&value(name, b))
}

#[test]
fn date_time() {
    check(
        "dateTime",
        &[
            "2002-10-10T12:00:00",
            "2002-10-10T12:00:00Z",
            "2002-10-10T12:00:00-05:00",
            "2002-10-10T12:00:00.123+14:00",
            "2002-10-10T12:00:00.000000000000001Z",
            "-0001-01-01T00:00:00",
            "12345-01-01T00:00:00",
            "2002-10-10T24:00:00",
            "2000-02-29T00:00:00",
            "  2002-10-10T12:00:00Z  ",
            "2002-12-31T24:00:00",
            "2002-10-10T12:00:00-14:00",
            "2002-10-10T12:00:00+00:00",
            "2002-10-10T12:00:00-00:00",
        ],
        &[
            "2002-10-10",
            "2002-10-10T12:00",
            "02-10-10T12:00:00",
            "2002-13-10T12:00:00",
            "2002-00-10T12:00:00",
            "2002-10-00T12:00:00",
            "2002-02-30T12:00:00",
            "2001-02-29T00:00:00",
            "1900-02-29T00:00:00",
            "2002-04-31T00:00:00",
            "2002-10-10T25:00:00",
            "2002-10-10T24:00:01",
            "2002-10-10T24:01:00",
            "2002-10-10T12:60:00",
            "2002-10-10T12:00:60",
            "2002-10-10T12:00:00+15:00",
            "2002-10-10T12:00:00+14:30",
            "2002-10-10T12:00:00+05:60",
            "2002-10-10T12:00:00+5:00",
            "2002-10-10 12:00:00",
            "2002-10-10t12:00:00",
            "0000-01-01T00:00:00",
            "02002-10-10T12:00:00",
            "002-10-10T12:00:00",
            "2002-10-10T12:00:00.",
            "2002-10-10T12:00:00Z+01:00",
            "2002-10-10T12:00:00z",
            "2002-10-10T1:00:00",
            "2002-10-10T12:00:00 Z",
            "",
            "T12:00:00",
            "+2002-10-10T12:00:00",
        ],
    );
}

#[test]
fn leap_years_are_those_of_the_gregorian_calendar_whatever_the_year() {
    check(
        "date",
        &["2000-02-29", "2004-02-29", "2400-02-29", "-0001-02-29", "100000000000000000-02-29", "100000000000000004-02-29"],
        &["1900-02-29", "2100-02-29", "2001-02-29", "-0002-02-29", "100000000000000100-02-29", "100000000000000001-02-29"],
    );
    check("date", &["1999-12-31"], &["1999-12-32", "2000-02-30"]);
    // Years of more digits than are read are refused, not misread.
    assert!(!valid("date", "1000000000000000000-01-01"));
    assert!(valid("date", "999999999999999999-01-01"));
}

#[test]
fn date_time_equality_and_order() {
    assert!(same("dateTime", "2002-10-10T12:00:00Z", "2002-10-10T13:00:00+01:00"));
    assert!(same("dateTime", "2002-10-10T12:00:00Z", "2002-10-10T12:00:00-00:00"));
    assert!(same("dateTime", "2002-10-10T12:00:00", "2002-10-10T12:00:00.000"));
    assert!(same("dateTime", "2002-10-10T24:00:00", "2002-10-11T00:00:00"));
    assert!(same("dateTime", "2002-12-31T24:00:00", "2003-01-01T00:00:00"));
    assert!(same("dateTime", "2002-10-10T23:30:00-01:00", "2002-10-11T00:30:00Z"));
    assert!(same("dateTime", "2002-10-10T00:30:00+01:00", "2002-10-09T23:30:00Z"), "across a day");
    assert!(same("dateTime", "2000-03-01T00:00:00+01:00", "2000-02-29T23:00:00Z"), "across a leap day");
    assert!(same("dateTime", "12345-01-01T00:00:00+01:00", "12344-12-31T23:00:00Z"), "across a year far from chrono's");
    assert!(same("dateTime", "-0001-01-01T00:00:00+01:00", "-0002-12-31T23:00:00Z"), "across 1 BCE");
    assert!(!same("dateTime", "2002-10-10T12:00:00Z", "2002-10-10T12:00:01Z"));
    assert_eq!(order("dateTime", "2002-10-10T12:00:00Z", "2002-10-10T12:00:01Z"), Some(Ordering::Less));
    assert_eq!(order("dateTime", "2002-10-10T12:00:00.5Z", "2002-10-10T12:00:00.25Z"), Some(Ordering::Greater));
    assert_eq!(order("dateTime", "2002-10-10T12:00:00+14:00", "2002-10-10T12:00:00-14:00"), Some(Ordering::Less));
    assert_eq!(order("dateTime", "-0001-12-31T23:59:59Z", "0001-01-01T00:00:00Z"), Some(Ordering::Less));
    assert_eq!(order("dateTime", "99999-01-01T00:00:00Z", "100000-01-01T00:00:00Z"), Some(Ordering::Less));
    assert_eq!(order("dateTime", "999999999999999999-01-01T00:00:00Z", "999999999999999998-12-31T23:59:59Z"), Some(Ordering::Greater));
}

#[test]
fn a_value_with_a_zone_and_one_without_are_ordered_only_if_every_zone_agrees() {
    // Within fourteen hours either way they cannot be told apart.
    assert_eq!(order("dateTime", "2002-10-10T00:00:00Z", "2002-10-10T01:00:00"), None);
    assert_eq!(order("dateTime", "2002-10-10T01:00:00", "2002-10-10T00:00:00Z"), None);
    assert_eq!(order("dateTime", "2002-10-10T00:00:00Z", "2002-10-10T14:00:00"), None);
    // Beyond it they can.
    assert_eq!(order("dateTime", "2002-10-10T00:00:00Z", "2002-10-12T00:00:00"), Some(Ordering::Less));
    assert_eq!(order("dateTime", "2002-10-12T00:00:00", "2002-10-10T00:00:00Z"), Some(Ordering::Greater));
    assert_eq!(order("dateTime", "2002-10-12T00:00:00Z", "2002-10-10T00:00:00"), Some(Ordering::Greater));
    assert!(!same("dateTime", "2002-10-10T12:00:00", "2002-10-10T12:00:00Z"), "not comparable is not equal");
    assert!(same("dateTime", "2002-10-10T12:00:00", "2002-10-10T12:00:00"));
}

#[test]
fn canonical_forms() {
    for (name, lexical, canonical) in [
        ("dateTime", "2002-10-10T12:00:00.500+01:00", "2002-10-10T12:00:00.5+01:00"),
        ("dateTime", "2002-10-10T12:00:00-00:00", "2002-10-10T12:00:00Z"),
        ("dateTime", "2002-10-10T24:00:00", "2002-10-11T00:00:00"),
        ("dateTime", "-0001-01-01T05:06:07.0", "-0001-01-01T05:06:07"),
        ("dateTime", "12345-01-01T00:00:00", "12345-01-01T00:00:00"),
        ("date", "2002-10-10+05:30", "2002-10-10+05:30"),
        ("date", "2002-10-10-14:00", "2002-10-10-14:00"),
        ("time", "24:00:00", "00:00:00"),
        ("time", "13:20:00.0Z", "13:20:00Z"),
        ("gYearMonth", "2002-10", "2002-10"),
        ("gYear", "-0044", "-0044"),
        ("gMonthDay", "--02-29", "--02-29"),
        ("gDay", "---31Z", "---31Z"),
        ("gMonth", "--12", "--12"),
    ] {
        assert_eq!(value(name, lexical).canonical(), canonical, "{name} {lexical}");
    }
}

#[test]
fn time() {
    check(
        "time",
        &["00:00:00", "13:20:00", "13:20:00Z", "13:20:00-05:00", "12:00:00.123", "24:00:00", "23:59:59.999999999"],
        &["13:20", "13:20:0", "25:00:00", "24:00:01", "12:60:00", "12:00:60", "1:20:00", "13:20:00+15:00", "13:20:00.", "T13:20:00", "13-20-00", ""],
    );
    assert!(same("time", "12:00:00Z", "13:00:00+01:00"));
    assert!(same("time", "24:00:00", "00:00:00"));
    assert_eq!(order("time", "12:00:00Z", "12:00:01Z"), Some(Ordering::Less));
    assert_eq!(
        order("time", "00:30:00+01:00", "23:30:00Z"),
        Some(Ordering::Less),
        "times are values on one day, 31 December 1972: 00:30+01:00 is the evening before"
    );
}

#[test]
fn date() {
    check(
        "date",
        &["2002-10-10", "2002-10-10Z", "2002-10-10+05:30", "-0001-12-31", "12345-06-07", "2002-10-10-14:00"],
        &["2002-10", "2002-10-10T00:00:00", "2002-1-10", "2002-10-1", "2002-10-32", "2002/10/10", "20021010", "2002-10-10+14:01", ""],
    );
    assert!(same("date", "2002-10-10Z", "2002-10-10Z"));
    assert!(!same("date", "2002-10-10Z", "2002-10-10+01:00"), "midnight in a zone is another instant");
    assert_eq!(order("date", "2002-10-09Z", "2002-10-10Z"), Some(Ordering::Less));
}

#[test]
fn the_gregorian_types() {
    check("gYearMonth", &["2002-10", "2002-10Z", "-0001-01", "12345-12+01:00"], &["2002", "2002-13", "2002-00", "2002-1", "2002-10-10", "02-10", ""]);
    check("gYear", &["2002", "2002Z", "-0044", "12345", "2002+01:00"], &["02", "0000", "002", "02002", "2002-10", "20a2", ""]);
    check(
        "gMonthDay",
        &["--02-29", "--12-31", "--01-01Z", "--04-30"],
        &["--02-30", "--04-31", "--13-01", "--00-01", "--1-01", "-02-29", "02-29", "--02", ""],
    );
    check("gDay", &["---01", "---31", "---15Z", "---31-05:00"], &["---00", "---32", "--01", "---1", "01", ""]);
    check("gMonth", &["--01", "--12", "--06Z", "--06+05:00"], &["--00", "--13", "--1", "-01", "--01--", "01", ""]);
    assert_eq!(order("gYear", "2000", "2001"), Some(Ordering::Less));
    assert_eq!(order("gYearMonth", "2000-12", "2001-01"), Some(Ordering::Less));
    assert_eq!(order("gMonthDay", "--02-28", "--02-29"), Some(Ordering::Less));
    assert_eq!(order("gMonthDay", "--12-01", "--02-29"), Some(Ordering::Greater));
    assert_eq!(order("gDay", "---05", "---15"), Some(Ordering::Less));
    assert_eq!(order("gMonth", "--05", "--04"), Some(Ordering::Greater));
    assert!(same("gYear", "2000Z", "2000+00:00"));
    // Values of different types are never ordered or equal.
    assert_eq!(value("gYear", "2000").compare(&value("gMonth", "--01")), None);
    assert!(!value("date", "2000-01-01").equals(&value("dateTime", "2000-01-01T00:00:00")));
}

#[test]
fn duration() {
    check(
        "duration",
        &[
            "P1Y",
            "P1Y2M3DT4H5M6.7S",
            "-P1D",
            "PT0S",
            "P0Y",
            "PT1.5S",
            "P1M",
            "PT36H",
            "P1DT12H",
            "P0D",
            "-P0D",
            "P100000000000000000Y",
            "PT0.000000001S",
            "  P1D  ",
        ],
        &[
            "P",
            "PT",
            "P1YT",
            "1Y",
            "P1Y-2M",
            "P1H",
            "PT1D",
            "P1M1Y",
            "P-1Y",
            "P1.5Y",
            "PT.5S",
            "PT5.S",
            "P1S",
            "PT1M1H",
            "-",
            "p1y",
            "P1Y2",
            "P1Y2M3D4H",
            "PT1,5S",
            "+P1Y",
            "P1000000000000000000Y",
            "",
        ],
    );
    assert!(same("duration", "P1Y", "P12M"));
    assert!(same("duration", "PT24H", "P1D"));
    assert!(same("duration", "PT90M", "PT1H30M"));
    assert!(same("duration", "PT1.50S", "PT1.5S"));
    assert!(same("duration", "-P0D", "PT0S"));
    assert!(!same("duration", "P1M", "P30D"));
    assert!(!same("duration", "P1Y", "P365D"));
    assert!(!same("duration", "P1D", "-P1D"));
    for (lexical, canonical) in [
        ("P1Y2M3DT4H5M6.7S", "P1Y2M3DT4H5M6.7S"),
        ("PT36H", "P1DT12H"),
        ("P14M", "P1Y2M"),
        ("-P1D", "-P1D"),
        ("P0D", "PT0S"),
        ("-P0Y", "PT0S"),
        ("PT0.5S", "PT0.5S"),
        ("PT60S", "PT1M"),
        ("PT3600S", "PT1H"),
        ("P1DT0.25S", "P1DT0.25S"),
        ("PT100H", "P4DT4H"),
        ("-P1Y1M1DT1H1M1.5S", "-P1Y1M1DT1H1M1.5S"),
    ] {
        assert_eq!(value("duration", lexical).canonical(), canonical, "{lexical}");
    }
}

#[test]
fn the_order_of_durations_is_partial() {
    // From the specification: P1Y is longer than P364D and shorter than P367D, and in no order with P365D or P366D (a year is 365 or
    // 366 days depending on the date it starts on); P1M and P30D are in none either.
    assert_eq!(order("duration", "P1Y", "P364D"), Some(Ordering::Greater));
    assert_eq!(order("duration", "P364D", "P1Y"), Some(Ordering::Less));
    assert_eq!(order("duration", "P1Y", "P367D"), Some(Ordering::Less));
    assert_eq!(order("duration", "P1Y", "P366D"), None);
    assert_eq!(order("duration", "P1Y", "P365D"), None);
    assert_eq!(order("duration", "P1M", "P30D"), None);
    // Two months are 61 or 62 days from the first of a month: P62D is never shorter and sometimes equal, P63D is always longer.
    assert_eq!(order("duration", "P2M", "P62D"), None);
    assert_eq!(order("duration", "P2M", "P63D"), Some(Ordering::Less));
    assert_eq!(order("duration", "P2M", "P58D"), Some(Ordering::Greater));
    assert_eq!(order("duration", "P1M", "P27D"), Some(Ordering::Greater));
    assert_eq!(order("duration", "P1M", "P32D"), Some(Ordering::Less));
    assert_eq!(order("duration", "P2M", "P1M"), Some(Ordering::Greater));
    assert_eq!(order("duration", "PT1H", "PT59M"), Some(Ordering::Greater));
    assert_eq!(order("duration", "PT0.5S", "PT0.25S"), Some(Ordering::Greater));
    assert_eq!(order("duration", "-P1D", "P1D"), Some(Ordering::Less));
    assert_eq!(order("duration", "-PT0.5S", "-PT0.25S"), Some(Ordering::Less), "a negative fraction");
    assert_eq!(order("duration", "-P1M", "-P27D"), Some(Ordering::Less));
    assert_eq!(order("duration", "P1Y", "P1Y"), Some(Ordering::Equal));
    assert_eq!(order("duration", "PT24H", "P1D"), Some(Ordering::Equal));
    // Huge durations do not break the arithmetic.
    assert_eq!(order("duration", "P100000000000000000Y", "P99999999999999999Y"), Some(Ordering::Greater));
    assert_eq!(order("duration", "P100000000000000000D", "P99999999999999999D"), Some(Ordering::Greater));
}
