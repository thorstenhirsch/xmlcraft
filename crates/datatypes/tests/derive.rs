//! Types made by restriction, as a list and as a union: how the facets decide, and which facets a schema may not write.

use std::sync::Arc;

use xmlcraft_datatypes::{
    ExpandedName, FacetSpec, NamespaceContext, NoNamespaces, Setting, SimpleType, Value, Variety, ViolationKind, list_of, restrict, union_of,
};

fn builtin(name: &str) -> Arc<SimpleType> {
    SimpleType::builtin(name).unwrap_or_else(|| panic!("no built-in type {name}"))
}

fn make(base: &Arc<SimpleType>, spec: FacetSpec) -> Arc<SimpleType> {
    restrict(base, None, &spec, &NoNamespaces).unwrap_or_else(|e| panic!("the restriction should be valid: {e:?}"))
}

fn errors(base: &Arc<SimpleType>, spec: FacetSpec) -> Vec<String> {
    restrict(base, None, &spec, &NoNamespaces).expect_err("the restriction should be refused")
}

fn s(value: &str) -> Option<Setting> {
    Some(Setting::new(value))
}

fn kind(t: &SimpleType, lexical: &str) -> Option<ViolationKind> {
    t.validate(lexical, &NoNamespaces).err().map(|v| v.kind)
}

fn ok(t: &SimpleType, lexical: &str) -> bool {
    t.validate(lexical, &NoNamespaces).is_ok()
}

fn has(errors: &[String], what: &str) -> bool {
    errors.iter().any(|e| e.contains(what))
}

#[test]
fn length_is_counted_in_the_units_of_the_type() {
    let string = builtin("string");
    let three = make(&string, FacetSpec { length: s("3"), ..FacetSpec::default() });
    assert!(ok(&three, "abc") && ok(&three, "a\u{1D11E}c"), "a character outside the BMP is one");
    assert_eq!(kind(&three, "ab"), Some(ViolationKind::Length));
    assert_eq!(kind(&three, "abcd"), Some(ViolationKind::Length));
    let range = make(&string, FacetSpec { min_length: s("2"), max_length: s("4"), ..FacetSpec::default() });
    assert_eq!(
        (kind(&range, "a"), kind(&range, "ab"), kind(&range, "abcd"), kind(&range, "abcde")),
        (Some(ViolationKind::MinLength), None, None, Some(ViolationKind::MaxLength))
    );
    // Octets of binary data.
    let two_octets = make(&builtin("hexBinary"), FacetSpec { length: s("2"), ..FacetSpec::default() });
    assert!(ok(&two_octets, "0FB7") && !ok(&two_octets, "0F") && !ok(&two_octets, "0FB7AA"));
    let b64 = make(&builtin("base64Binary"), FacetSpec { max_length: s("3"), ..FacetSpec::default() });
    assert!(ok(&b64, "QUJD") && !ok(&b64, "QUJDRA=="));
    // The length of the value, after the white space has been dealt with.
    let token = make(&builtin("token"), FacetSpec { length: s("3"), ..FacetSpec::default() });
    assert!(ok(&token, "  a   b  ") && !ok(&token, "abc d"), "'a b' has three characters");
    let preserved = make(&string, FacetSpec { length: s("5"), ..FacetSpec::default() });
    assert!(ok(&preserved, " a b "));
    assert!(ok(&make(&builtin("anyURI"), FacetSpec { max_length: s("5"), ..FacetSpec::default() }), "a/b/c"));
}

#[test]
fn patterns_are_ored_in_a_step_and_anded_between_steps() {
    let digits = make(&builtin("string"), FacetSpec { patterns: vec!["[0-9]+".into()], ..FacetSpec::default() });
    assert!(ok(&digits, "123") && !ok(&digits, "12a") && !ok(&digits, ""));
    assert_eq!(kind(&digits, "12a"), Some(ViolationKind::Pattern));
    let either = make(&builtin("string"), FacetSpec { patterns: vec!["a+".into(), "b+".into()], ..FacetSpec::default() });
    assert!(ok(&either, "aaa") && ok(&either, "bb") && !ok(&either, "ab"));
    let low = make(&digits, FacetSpec { patterns: vec!["[0-5]+".into()], ..FacetSpec::default() });
    assert!(ok(&low, "0125") && !ok(&low, "126"), "both steps have to match");
    assert_eq!(kind(&low, "126"), Some(ViolationKind::Pattern));
    // A pattern looks at the lexical form after white space has been dealt with, not at the value.
    let price = make(&builtin("decimal"), FacetSpec { patterns: vec!["[0-9]+\\.[0-9]{2}".into()], ..FacetSpec::default() });
    assert!(ok(&price, "12.50") && ok(&price, "  12.50  ") && !ok(&price, "12.5") && !ok(&price, "12.500"));
    assert!(ok(&make(&builtin("string"), FacetSpec { patterns: vec!["[a-z]+".into()], ..FacetSpec::default() }), "abc"));
    // A bad pattern is a mistake of the schema.
    let e = errors(&builtin("string"), FacetSpec { patterns: vec!["(a".into()], ..FacetSpec::default() });
    assert!(has(&e, "pattern '(a'"), "{e:?}");
}

#[test]
fn an_enumeration_is_a_set_of_values_not_of_spellings() {
    let numbers = make(&builtin("decimal"), FacetSpec { enumeration: vec!["1".into(), "2.5".into()], ..FacetSpec::default() });
    assert!(ok(&numbers, "1.0") && ok(&numbers, "+2.50") && ok(&numbers, "001") && !ok(&numbers, "3"));
    assert_eq!(kind(&numbers, "3"), Some(ViolationKind::Enumeration));
    let colours = make(&builtin("token"), FacetSpec { enumeration: vec!["red".into(), "green".into()], ..FacetSpec::default() });
    assert!(ok(&colours, " red ") && !ok(&colours, "Red"));
    let times = make(&builtin("dateTime"), FacetSpec { enumeration: vec!["2002-10-10T12:00:00Z".into()], ..FacetSpec::default() });
    assert!(ok(&times, "2002-10-10T13:00:00+01:00") && !ok(&times, "2002-10-10T12:00:00"));
    let floats = make(&builtin("double"), FacetSpec { enumeration: vec!["NaN".into(), "0".into()], ..FacetSpec::default() });
    assert!(ok(&floats, "NaN") && ok(&floats, "-0"));
    // The values have to be values of the base type.
    let e = errors(&builtin("integer"), FacetSpec { enumeration: vec!["1".into(), "x".into()], ..FacetSpec::default() });
    assert!(has(&e, "enumeration: 'x'"), "{e:?}");
    let e = errors(&builtin("byte"), FacetSpec { enumeration: vec!["300".into()], ..FacetSpec::default() });
    assert!(has(&e, "enumeration: '300'"), "{e:?}");
}

#[test]
fn qname_enumerations_compare_the_namespaces_not_the_prefixes() {
    struct Ns;
    impl NamespaceContext for Ns {
        fn resolve(&self, prefix: Option<&str>) -> Option<String> {
            match prefix {
                Some("a" | "b") => Some("urn:x".to_owned()),
                Some("c") => Some("urn:y".to_owned()),
                _ => None,
            }
        }
    }
    let t = restrict(&builtin("QName"), None, &FacetSpec { enumeration: vec!["a:n".into()], ..FacetSpec::default() }, &Ns).expect("valid");
    assert!(t.validate("b:n", &Ns).is_ok());
    assert!(t.validate("c:n", &Ns).is_err() && t.validate("b:m", &Ns).is_err());
}

#[test]
fn ranges() {
    let integer = builtin("integer");
    let t = make(&integer, FacetSpec { min_inclusive: s("1"), max_inclusive: s("10"), ..FacetSpec::default() });
    assert!(ok(&t, "1") && ok(&t, "10") && !ok(&t, "0") && !ok(&t, "11"));
    assert_eq!((kind(&t, "0"), kind(&t, "11")), (Some(ViolationKind::MinInclusive), Some(ViolationKind::MaxInclusive)));
    let t = make(&integer, FacetSpec { min_exclusive: s("1"), max_exclusive: s("10"), ..FacetSpec::default() });
    assert!(ok(&t, "2") && ok(&t, "9") && !ok(&t, "1") && !ok(&t, "10"));
    assert_eq!((kind(&t, "1"), kind(&t, "10")), (Some(ViolationKind::MinExclusive), Some(ViolationKind::MaxExclusive)));
    let t = make(&builtin("decimal"), FacetSpec { min_inclusive: s("0.5"), max_exclusive: s("1"), ..FacetSpec::default() });
    assert!(ok(&t, "0.5") && ok(&t, "0.99999999999999999999") && !ok(&t, "1.0") && !ok(&t, "0.4999"));
    let t = make(&builtin("double"), FacetSpec { min_inclusive: s("-1"), max_inclusive: s("1"), ..FacetSpec::default() });
    assert!(ok(&t, "0.5") && !ok(&t, "2") && !ok(&t, "NaN") && !ok(&t, "INF"));
    // Dates: a date and a bound that cannot be told apart (one has a zone, the other none) are not in the range.
    let t = make(&builtin("date"), FacetSpec { min_inclusive: s("2000-01-01Z"), max_inclusive: s("2000-12-31Z"), ..FacetSpec::default() });
    assert!(ok(&t, "2000-06-15Z") && !ok(&t, "1999-12-30Z") && !ok(&t, "2001-01-01Z"));
    assert!(ok(&t, "2000-06-15"), "months away from the bounds, a zone does not matter");
    let e = t.validate("2000-01-01", &NoNamespaces).expect_err("not comparable with the lower bound");
    assert!(e.message.contains("cannot be compared"), "{e}");
    let t = make(&builtin("duration"), FacetSpec { max_inclusive: s("P1Y"), ..FacetSpec::default() });
    assert!(ok(&t, "P364D") && ok(&t, "P12M") && !ok(&t, "P367D") && !ok(&t, "P365D"), "P365D cannot be told from P1Y");
    let t = make(&builtin("time"), FacetSpec { min_inclusive: s("08:00:00Z"), ..FacetSpec::default() });
    assert!(ok(&t, "09:00:00Z") && !ok(&t, "07:00:00Z"));
}

#[test]
fn digits() {
    let t = make(&builtin("decimal"), FacetSpec { total_digits: s("5"), fraction_digits: s("2"), ..FacetSpec::default() });
    for good in ["123.45", "0", "100", "99999", "999.99", "-12.5", "0.05", "00123.4500", ".5"] {
        assert!(ok(&t, good), "{good}");
    }
    assert_eq!(kind(&t, "1234.56"), Some(ViolationKind::TotalDigits));
    assert_eq!(kind(&t, "100000"), Some(ViolationKind::TotalDigits));
    assert_eq!(kind(&t, "1.234"), Some(ViolationKind::FractionDigits));
    assert_eq!(kind(&t, "0.001"), Some(ViolationKind::FractionDigits));
    let one = make(&builtin("decimal"), FacetSpec { total_digits: s("1"), ..FacetSpec::default() });
    assert!(ok(&one, "7") && ok(&one, "0.5") && !ok(&one, "0.05"), "the zero in 0.05 is a digit");
    let whole = make(&builtin("decimal"), FacetSpec { fraction_digits: s("0"), ..FacetSpec::default() });
    assert!(ok(&whole, "5.0") && !ok(&whole, "5.1"));
    // The integer types fix the fraction digits to 0.
    let e = errors(&builtin("integer"), FacetSpec { fraction_digits: s("1"), ..FacetSpec::default() });
    assert!(has(&e, "fractionDigits"), "{e:?}");
    let t = make(&builtin("integer"), FacetSpec { total_digits: s("3"), ..FacetSpec::default() });
    assert!(ok(&t, "999") && !ok(&t, "1000") && ok(&t, "-999"));
}

#[test]
fn white_space_may_only_become_stricter() {
    let string = builtin("string");
    let replace = make(&string, FacetSpec { white_space: s("replace"), ..FacetSpec::default() });
    assert_eq!(replace.validate("a\tb", &NoNamespaces).expect("valid").canonical(), "a b");
    let collapse = make(&replace, FacetSpec { white_space: s("collapse"), ..FacetSpec::default() });
    assert_eq!(collapse.validate("  a \t b ", &NoNamespaces).expect("valid").canonical(), "a b");
    let e = errors(&collapse, FacetSpec { white_space: s("preserve"), ..FacetSpec::default() });
    assert!(has(&e, "less strict"), "{e:?}");
    let e = errors(&builtin("token"), FacetSpec { white_space: s("replace"), ..FacetSpec::default() });
    assert!(has(&e, "less strict"), "{e:?}");
    let e = errors(&builtin("integer"), FacetSpec { white_space: s("preserve"), ..FacetSpec::default() });
    assert!(has(&e, "less strict"), "{e:?}");
    let e = errors(&string, FacetSpec { white_space: s("squeeze"), ..FacetSpec::default() });
    assert!(has(&e, "preserve, replace or collapse"), "{e:?}");
    // The length is counted after normalisation.
    let two = make(&collapse, FacetSpec { length: s("3"), ..FacetSpec::default() });
    assert!(ok(&two, "  a   b "));
}

#[test]
fn a_fixed_facet_cannot_be_changed() {
    let base = make(&builtin("string"), FacetSpec { max_length: Some(Setting::fixed("10")), min_inclusive: None, ..FacetSpec::default() });
    let same = make(&base, FacetSpec { max_length: s("10"), ..FacetSpec::default() });
    assert!(ok(&same, "0123456789"));
    let e = errors(&base, FacetSpec { max_length: s("5"), ..FacetSpec::default() });
    assert!(has(&e, "maxLength"), "{e:?}");
    // The fixed flag is kept through restrictions that do not touch the facet.
    let narrower = make(&base, FacetSpec { min_length: s("1"), ..FacetSpec::default() });
    let e = errors(&narrower, FacetSpec { max_length: s("5"), ..FacetSpec::default() });
    assert!(has(&e, "maxLength"), "{e:?}");
    let ws = make(&builtin("string"), FacetSpec { white_space: Some(Setting::fixed("collapse")), ..FacetSpec::default() });
    assert!(
        restrict(&ws, None, &FacetSpec { white_space: s("collapse"), ..FacetSpec::default() }, &NoNamespaces).is_ok(),
        "the same value is not a change"
    );
    let ranged = make(&builtin("integer"), FacetSpec { max_inclusive: Some(Setting::fixed("100")), ..FacetSpec::default() });
    assert!(restrict(&ranged, None, &FacetSpec { max_inclusive: s("100"), ..FacetSpec::default() }, &NoNamespaces).is_ok());
    let e = errors(&ranged, FacetSpec { max_inclusive: s("50"), ..FacetSpec::default() });
    assert!(has(&e, "maxInclusive: the base type fixes it"), "{e:?}");
}

#[test]
fn facets_that_do_not_apply_are_refused() {
    for (base, spec, facet) in [
        ("integer", FacetSpec { length: s("1"), ..FacetSpec::default() }, "length"),
        ("integer", FacetSpec { min_length: s("1"), ..FacetSpec::default() }, "minLength"),
        ("boolean", FacetSpec { max_length: s("1"), ..FacetSpec::default() }, "maxLength"),
        ("string", FacetSpec { min_inclusive: s("a"), ..FacetSpec::default() }, "minInclusive"),
        ("string", FacetSpec { max_exclusive: s("a"), ..FacetSpec::default() }, "maxExclusive"),
        ("hexBinary", FacetSpec { max_inclusive: s("00"), ..FacetSpec::default() }, "maxInclusive"),
        ("boolean", FacetSpec { min_exclusive: s("true"), ..FacetSpec::default() }, "minExclusive"),
        ("string", FacetSpec { total_digits: s("3"), ..FacetSpec::default() }, "totalDigits"),
        ("double", FacetSpec { fraction_digits: s("3"), ..FacetSpec::default() }, "fractionDigits"),
        ("date", FacetSpec { total_digits: s("3"), ..FacetSpec::default() }, "totalDigits"),
    ] {
        let e = errors(&builtin(base), spec);
        assert!(has(&e, &format!("{facet} does not apply")), "{base} {facet}: {e:?}");
    }
    // But these do.
    make(&builtin("double"), FacetSpec { max_inclusive: s("1e3"), patterns: vec!["[0-9e]+".into()], ..FacetSpec::default() });
    make(&builtin("boolean"), FacetSpec { patterns: vec!["true|1".into()], ..FacetSpec::default() });
    make(&builtin("QName"), FacetSpec { patterns: vec!["x".into()], ..FacetSpec::default() });
    make(&builtin("anyURI"), FacetSpec { length: s("3"), ..FacetSpec::default() });
}

#[test]
fn the_values_of_facets_must_make_sense() {
    let string = builtin("string");
    for (spec, what) in [
        (FacetSpec { length: s("-1"), ..FacetSpec::default() }, "length: '-1' is not a non-negative integer"),
        (FacetSpec { min_length: s("abc"), ..FacetSpec::default() }, "minLength: 'abc'"),
        (FacetSpec { max_length: s("1.5"), ..FacetSpec::default() }, "maxLength: '1.5'"),
        (FacetSpec { length: s(""), ..FacetSpec::default() }, "length: ''"),
        (FacetSpec { length: s("99999999999999999999999999999999999999999"), ..FacetSpec::default() }, "length:"),
    ] {
        assert!(has(&errors(&string, spec), what), "{what}");
    }
    let decimal = builtin("decimal");
    assert!(has(&errors(&decimal, FacetSpec { total_digits: s("0"), ..FacetSpec::default() }), "totalDigits: '0' is not a positive integer"));
    assert!(has(&errors(&decimal, FacetSpec { fraction_digits: s("-1"), ..FacetSpec::default() }), "fractionDigits"));
    assert!(has(
        &errors(&decimal, FacetSpec { max_inclusive: s("abc"), ..FacetSpec::default() }),
        "maxInclusive: 'abc' is not a valid value of the base type"
    ));
    // Several mistakes are all reported.
    let e = errors(&string, FacetSpec { length: s("x"), min_length: s("y"), ..FacetSpec::default() });
    assert_eq!(e.len(), 2, "{e:?}");
}

#[test]
fn facets_must_agree_with_each_other_and_with_the_base_type() {
    let string = builtin("string");
    assert!(has(&errors(&string, FacetSpec { min_length: s("5"), max_length: s("2"), ..FacetSpec::default() }), "greater than maxLength"));
    assert!(has(&errors(&string, FacetSpec { length: s("3"), min_length: s("4"), ..FacetSpec::default() }), "less than minLength"));
    assert!(has(&errors(&string, FacetSpec { length: s("3"), max_length: s("2"), ..FacetSpec::default() }), "greater than maxLength"));
    make(&string, FacetSpec { length: s("3"), min_length: s("1"), max_length: s("5"), ..FacetSpec::default() });
    make(&string, FacetSpec { length: s("3"), min_length: s("3"), max_length: s("3"), ..FacetSpec::default() });
    let base = make(&string, FacetSpec { min_length: s("2"), max_length: s("8"), ..FacetSpec::default() });
    assert!(has(&errors(&base, FacetSpec { min_length: s("1"), ..FacetSpec::default() }), "less than the base type's 2"));
    assert!(has(&errors(&base, FacetSpec { max_length: s("9"), ..FacetSpec::default() }), "greater than the base type's 8"));
    assert!(has(&errors(&base, FacetSpec { min_length: s("9"), ..FacetSpec::default() }), "greater than maxLength"), "against the base's maxLength");
    assert!(
        has(&errors(&base, FacetSpec { max_length: s("1"), ..FacetSpec::default() }), "less than the base type's 2")
            || has(&errors(&base, FacetSpec { max_length: s("1"), ..FacetSpec::default() }), "greater than maxLength")
    );
    make(&base, FacetSpec { min_length: s("3"), max_length: s("7"), ..FacetSpec::default() });
    let exact = make(&string, FacetSpec { length: s("4"), ..FacetSpec::default() });
    assert!(has(&errors(&exact, FacetSpec { length: s("5"), ..FacetSpec::default() }), "is not the base type's 4"));
    // Digits.
    let d = make(&builtin("decimal"), FacetSpec { total_digits: s("6"), fraction_digits: s("3"), ..FacetSpec::default() });
    assert!(has(&errors(&d, FacetSpec { total_digits: s("7"), ..FacetSpec::default() }), "greater than the base type's 6"));
    assert!(has(&errors(&d, FacetSpec { fraction_digits: s("4"), ..FacetSpec::default() }), "greater than the base type's 3"));
    assert!(
        has(&errors(&d, FacetSpec { total_digits: s("2"), ..FacetSpec::default() }), "greater than totalDigits"),
        "fractionDigits 3 > totalDigits 2"
    );
    assert!(has(
        &errors(&builtin("decimal"), FacetSpec { total_digits: s("2"), fraction_digits: s("3"), ..FacetSpec::default() }),
        "greater than totalDigits 2"
    ));
    // Ranges.
    let integer = builtin("integer");
    assert!(has(
        &errors(&integer, FacetSpec { min_inclusive: s("5"), max_inclusive: s("4"), ..FacetSpec::default() }),
        "lower bound 5 is greater than the upper bound 4"
    ));
    assert!(has(&errors(&integer, FacetSpec { min_exclusive: s("5"), max_inclusive: s("5"), ..FacetSpec::default() }), "leave no value"));
    make(&integer, FacetSpec { min_inclusive: s("5"), max_inclusive: s("5"), ..FacetSpec::default() });
    assert!(has(&errors(&integer, FacetSpec { max_inclusive: s("5"), max_exclusive: s("6"), ..FacetSpec::default() }), "cannot both be given"));
    assert!(has(&errors(&integer, FacetSpec { min_inclusive: s("5"), min_exclusive: s("4"), ..FacetSpec::default() }), "cannot both be given"));
    let byte = builtin("byte");
    assert!(has(
        &errors(&byte, FacetSpec { max_inclusive: s("128"), ..FacetSpec::default() }),
        "maxInclusive: '128' is not a valid value of the base type"
    ));
    make(&byte, FacetSpec { max_inclusive: s("100"), ..FacetSpec::default() });
    let narrow = make(&integer, FacetSpec { min_inclusive: s("0"), max_inclusive: s("10"), ..FacetSpec::default() });
    assert!(has(&errors(&narrow, FacetSpec { max_inclusive: s("11"), ..FacetSpec::default() }), "maxInclusive: '11'"));
    assert!(has(&errors(&narrow, FacetSpec { min_inclusive: s("-1"), ..FacetSpec::default() }), "minInclusive: '-1'"));
    // The bounds of dates that cannot be ordered.
    let e = errors(&builtin("date"), FacetSpec { min_inclusive: s("2000-01-01Z"), max_inclusive: s("2000-01-01"), ..FacetSpec::default() });
    assert!(has(&e, "cannot be compared"), "{e:?}");
}

#[test]
fn a_restriction_remembers_where_it_comes_from() {
    let named = restrict(
        &builtin("integer"),
        Some(ExpandedName::new(Some("urn:x"), "Percent")),
        &FacetSpec { min_inclusive: s("0"), max_inclusive: s("100"), ..FacetSpec::default() },
        &NoNamespaces,
    )
    .expect("valid");
    assert_eq!(named.name().map(|n| n.local.as_str()), Some("Percent"));
    assert!(named.is_derived_from(&builtin("integer")) && named.is_derived_from(&builtin("decimal")) && named.is_derived_from(&named));
    assert!(!builtin("integer").is_derived_from(&named));
    assert!(named.is_derived_from_builtin("decimal") && !named.is_derived_from_builtin("string"));
    assert!(
        builtin("ID").is_derived_from_builtin("NCName")
            && builtin("ID").is_derived_from_builtin("string")
            && builtin("ID").is_derived_from_builtin("anySimpleType")
    );
    assert!(builtin("int").is_derived_from_builtin("long") && !builtin("long").is_derived_from_builtin("int"));
    assert!(matches!(named.variety(), Variety::Atomic(_)));
    let message = named.validate("101", &NoNamespaces).expect_err("too big").message;
    assert!(message.contains("101") && message.contains("100"), "{message}");
    // An error names the type the value was meant to be.
    let e = named.validate("x", &NoNamespaces).expect_err("not a number");
    assert!(e.message.contains("Percent") || e.message.contains("integer"), "{e}");
}

#[test]
fn lists() {
    let numbers = list_of(&builtin("integer"), None).expect("a list");
    assert!(ok(&numbers, "1 2 3") && ok(&numbers, "") && ok(&numbers, "  7  ") && ok(&numbers, "1\n-2\t3"));
    let e = numbers.validate("1 a 3", &NoNamespaces).expect_err("the second item");
    assert!(e.message.contains("item 2"), "{e}");
    assert_eq!(numbers.validate("1  2", &NoNamespaces).expect("valid").length(), Some(2));
    assert!(numbers.item_type().is_some() && numbers.primitive().is_none());
    let three = make(&numbers, FacetSpec { length: s("3"), ..FacetSpec::default() });
    assert!(ok(&three, "1 2 3") && !ok(&three, "1 2") && !ok(&three, ""));
    assert_eq!(kind(&three, "1 2"), Some(ViolationKind::Length));
    let some = make(&numbers, FacetSpec { min_length: s("1"), max_length: s("2"), ..FacetSpec::default() });
    assert!(ok(&some, "1") && ok(&some, "1 2") && !ok(&some, "") && !ok(&some, "1 2 3"));
    // Enumerations of lists, and patterns on the whole list.
    let sets = make(&numbers, FacetSpec { enumeration: vec!["1 2".into(), "3".into()], ..FacetSpec::default() });
    assert!(ok(&sets, "01  +2") && ok(&sets, "3") && !ok(&sets, "2 1") && !ok(&sets, "1"));
    let patterned = make(&numbers, FacetSpec { patterns: vec!["[0-9]( [0-9])*".into()], ..FacetSpec::default() });
    assert!(ok(&patterned, "1 2 3") && !ok(&patterned, "10"));
    // A list of a restricted type checks its items.
    let small = make(&builtin("integer"), FacetSpec { max_inclusive: s("9"), ..FacetSpec::default() });
    let list = list_of(&small, None).expect("a list");
    assert!(ok(&list, "1 2 9") && !ok(&list, "1 10"));
    assert_eq!(kind(&list, "1 10"), Some(ViolationKind::MaxInclusive));
    // The built-in ones.
    assert!(ok(&builtin("NMTOKENS"), "a b") && !ok(&builtin("NMTOKENS"), ""));
    // Items are not lists, and facets for numbers do not apply to lists.
    let e = list_of(&numbers, None).expect_err("a list of lists");
    assert!(has(&e, "does not hold lists"), "{e:?}");
    let union_with_list = union_of(&[builtin("integer"), numbers.clone()], None).expect("a union");
    assert!(has(&list_of(&union_with_list, None).expect_err("holds a list"), "does not hold lists"));
    let e = errors(&numbers, FacetSpec { max_inclusive: s("5"), ..FacetSpec::default() });
    assert!(has(&e, "maxInclusive does not apply"), "{e:?}");
}

#[test]
fn unions() {
    let t = union_of(&[builtin("integer"), builtin("boolean")], None).expect("a union");
    assert!(matches!(t.validate("42", &NoNamespaces), Ok(Value::Decimal(_))));
    assert!(matches!(t.validate("true", &NoNamespaces), Ok(Value::Boolean(true))));
    assert!(matches!(t.validate("1", &NoNamespaces), Ok(Value::Decimal(_))), "the first member that accepts it");
    let e = t.validate("maybe", &NoNamespaces).expect_err("neither");
    assert_eq!(e.kind, ViolationKind::Union);
    assert!(e.message.contains("integer") && e.message.contains("boolean"), "{e}");
    assert_eq!(t.member_types().map(<[_]>::len), Some(2));
    // Members are tried in order, each with its own white space.
    let u = union_of(&[builtin("token"), builtin("integer")], None).expect("a union");
    assert!(matches!(u.validate(" 5 ", &NoNamespaces), Ok(Value::String(s)) if s == "5"));
    // Facets on the union: patterns on the lexical form, enumerations among the values of the members.
    let enumerated = make(&t, FacetSpec { enumeration: vec!["1".into(), "true".into()], ..FacetSpec::default() });
    assert!(ok(&enumerated, "1") && ok(&enumerated, "+01") && ok(&enumerated, "true") && !ok(&enumerated, "2") && !ok(&enumerated, "false"));
    assert_eq!(kind(&enumerated, "2"), Some(ViolationKind::Enumeration));
    let patterned = make(&t, FacetSpec { patterns: vec!["[0-9]+".into()], ..FacetSpec::default() });
    assert!(ok(&patterned, "12") && !ok(&patterned, "true"));
    assert_eq!(kind(&patterned, "true"), Some(ViolationKind::Pattern));
    // A union of lists is fine; length does not apply to a union.
    let lists = union_of(&[list_of(&builtin("integer"), None).expect("list"), builtin("string")], None).expect("a union");
    assert!(ok(&lists, "1 2") && ok(&lists, "x y"));
    let e = errors(&t, FacetSpec { length: s("1"), ..FacetSpec::default() });
    assert!(has(&e, "length does not apply"), "{e:?}");
    let e = errors(&t, FacetSpec { white_space: s("collapse"), ..FacetSpec::default() });
    assert!(has(&e, "whiteSpace does not apply"), "{e:?}");
    assert!(has(&union_of(&[], None).expect_err("empty"), "at least one member"));
}

#[test]
fn qualified_name_types_use_the_namespaces_in_scope_when_they_are_validated() {
    // A type derived from NCName by pattern, as a schema author would.
    let code = make(&builtin("NCName"), FacetSpec { patterns: vec!["[A-Z]{3}".into()], ..FacetSpec::default() });
    assert!(ok(&code, "ABC") && !ok(&code, "ABCD") && !ok(&code, "abc"));
    assert_eq!(kind(&code, "a b"), Some(ViolationKind::Lexical), "the lexical space of NCName comes first");
}
