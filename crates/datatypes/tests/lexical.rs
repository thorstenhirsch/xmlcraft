//! The lexical spaces of the built-in types: what is a value of each, and what it is the value of.

use xmlcraft_datatypes::{NamespaceContext, NoNamespaces, SimpleType, Value, ViolationKind};

fn ty(name: &str) -> std::sync::Arc<SimpleType> {
    SimpleType::builtin(name).unwrap_or_else(|| panic!("no built-in type {name}"))
}

fn valid(name: &str, lexical: &str) -> bool {
    ty(name).validate(lexical, &NoNamespaces).is_ok()
}

fn value(name: &str, lexical: &str) -> Value {
    ty(name).validate(lexical, &NoNamespaces).unwrap_or_else(|e| panic!("{lexical:?} should be a {name}: {e}"))
}

fn kind(name: &str, lexical: &str) -> ViolationKind {
    ty(name).validate(lexical, &NoNamespaces).expect_err(&format!("{lexical:?} should not be a {name}")).kind
}

fn check(name: &str, yes: &[&str], no: &[&str]) {
    for lexical in yes {
        assert!(valid(name, lexical), "{lexical:?} should be a {name}");
    }
    for lexical in no {
        assert!(!valid(name, lexical), "{lexical:?} should not be a {name}");
    }
}

#[test]
fn every_built_in_type_is_there_and_its_patterns_are_read() {
    let names = SimpleType::builtin_names();
    assert_eq!(names.len(), 45, "{names:?}");
    for name in [
        "anySimpleType",
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
        "NOTATION",
        "normalizedString",
        "token",
        "language",
        "NMTOKEN",
        "NMTOKENS",
        "Name",
        "NCName",
        "ID",
        "IDREF",
        "IDREFS",
        "ENTITY",
        "ENTITIES",
        "integer",
        "nonPositiveInteger",
        "negativeInteger",
        "long",
        "int",
        "short",
        "byte",
        "nonNegativeInteger",
        "unsignedLong",
        "unsignedInt",
        "unsignedShort",
        "unsignedByte",
        "positiveInteger",
    ] {
        assert!(names.contains(&name), "{name}");
    }
    // The types that are defined by a pattern have it (a pattern that could not be read would be missing and accept anything).
    for (name, bad) in [("language", "en_US"), ("NMTOKEN", "a b"), ("Name", "1a"), ("NCName", "a:b"), ("integer", "1.5")] {
        assert!(!valid(name, bad), "{name} {bad:?}");
    }
}

#[test]
fn string_types_and_their_white_space() {
    check("string", &["", " a  b ", "a\tb", "\u{1D11E}"], &[]);
    assert_eq!(value("string", " a\tb ").canonical(), " a\tb ");
    assert_eq!(value("normalizedString", " a\tb\nc\r\nd ").canonical(), " a b c  d ");
    assert_eq!(value("normalizedString", "a\rb").canonical(), "a b", "a carriage return alone is replaced too");
    assert_eq!(value("token", " a \t b\n\nc ").canonical(), "a b c");
    assert_eq!(value("anySimpleType", " x ").canonical(), " x ");
}

#[test]
fn boolean() {
    check("boolean", &["true", "false", "1", "0", " true ", "\ttrue\n"], &["", "True", "TRUE", "yes", "2", "-1", "tru", "true false", "01"]);
    assert!(matches!(value("boolean", "1"), Value::Boolean(true)));
    assert!(matches!(value("boolean", "false"), Value::Boolean(false)));
}

#[test]
fn decimal() {
    check(
        "decimal",
        &["0", "-0", "+0", "-1.5", "+.5", "5.", "007", "0.000", "  7 ", "123456789012345678901234567890.123456789012345678901234567890"],
        &["", ".", "+", "-", "1e5", "1,5", "0x1", "--1", "+-1", "NaN", "INF", "1 2", "1.2.3", "٣"],
    );
    assert_eq!(value("decimal", "+007.5000").canonical(), "7.5");
    assert_eq!(value("decimal", "-0.0").canonical(), "0");
    assert!(value("decimal", "1.0").equals(&value("decimal", "1")));
    assert!(!value("decimal", "1.01").equals(&value("decimal", "1")));
}

#[test]
fn float_and_double() {
    for name in ["float", "double"] {
        check(
            name,
            &["1", "-1.5E-3", "INF", "-INF", "NaN", "+1.0e+5", ".5", "5.", "0", "-0", "1e0", "12345678901234567890", "1e400", "1e-400"],
            &["+INF", "inf", "Infinity", "-Infinity", "nan", "1e", "e5", "", "0x1p3", "1_0", "1.5f", "1.5d", "+", ".", "1e+", "1 e5", "--1"],
        );
        assert!(value(name, "NaN").equals(&value(name, "NaN")), "NaN is NaN");
        assert!(value(name, "0").equals(&value(name, "-0")));
        assert!(value(name, "INF").equals(&value(name, "INF")));
        assert!(!value(name, "INF").equals(&value(name, "-INF")));
        assert_eq!(value(name, "NaN").compare(&value(name, "1")), None);
        assert_eq!(value(name, "1").compare(&value(name, "2")), Some(std::cmp::Ordering::Less));
    }
    assert_eq!(value("double", "150").canonical(), "1.5E2");
    assert_eq!(value("double", "0").canonical(), "0.0E0");
    assert_eq!(value("double", "-1e-3").canonical(), "-1.0E-3");
    assert_eq!(value("double", "INF").canonical(), "INF");
    assert_eq!(value("float", "0.1").canonical(), "1.0E-1");
    // A float has the precision of a float, a double that of a double.
    assert!(value("float", "0.1").equals(&value("float", "0.10000000149")));
    assert!(!value("double", "0.1").equals(&value("double", "0.10000000149")));
    // Too large is infinity, too small is zero.
    assert!(value("float", "1e400").equals(&value("float", "INF")));
    assert!(value("double", "1e-400").equals(&value("double", "0")));
}

#[test]
fn hex_binary() {
    check("hexBinary", &["", "0FB7", "0fb7", "00", "  AB  "], &["0FB", "GG", "0F B7", "0x0F", "é1"]);
    assert_eq!(value("hexBinary", "0fb7").canonical(), "0FB7");
    assert!(value("hexBinary", "0fb7").equals(&value("hexBinary", "0FB7")));
    assert_eq!(value("hexBinary", "0FB7").length(), Some(2));
}

#[test]
fn base64_binary() {
    check(
        "base64Binary",
        &["", "AAAA", "QQ==", "QUI=", "QUJD", "AA AA", "  QUJD  ", "AAAA AAAA", "+/+/"],
        &["QR==", "QQ=", "A", "====", "AA=A", "AAA", "QUJ=D", "QUI", "AA\u{0}A", "AAAA=", "QU=I", "QUJ=", "QQ==AAAA", "QUI=AAAA"],
    );
    assert_eq!(value("base64Binary", "QUJD").length(), Some(3));
    assert_eq!(value("base64Binary", "QUI=").length(), Some(2));
    assert_eq!(value("base64Binary", "QQ==").length(), Some(1));
    assert_eq!(value("base64Binary", "Q Q = =").canonical(), "QQ==", "spaces between the characters are allowed and not kept");
    assert!(value("base64Binary", "QUI=").equals(&value("base64Binary", " QUI= ")));
}

#[test]
fn any_uri() {
    check(
        "anyURI",
        &["", "http://example.org/a", "a/b", "../x", "urn:x:y", "%41", "a b", "#frag", "?q=1", "mailto:a@b.c", "é/ü", "x:y", "//host/p", "a#b"],
        &["%4", "%GG", "a#b#c", "1http://x", "a\u{1}b", ":x", "a%"],
    );
    assert_eq!(value("anyURI", " http://x ").canonical(), "http://x", "white space is collapsed");
    assert!(!value("anyURI", "http://x/a").equals(&value("anyURI", "http://x/A")));
}

struct Declared(&'static [(&'static str, &'static str)], Option<&'static str>);

impl NamespaceContext for Declared {
    fn resolve(&self, prefix: Option<&str>) -> Option<String> {
        match prefix {
            Some(p) => self.0.iter().find(|(k, _)| *k == p).map(|(_, v)| (*v).to_owned()),
            None => self.1.map(str::to_owned),
        }
    }
}

#[test]
fn qname_and_notation() {
    let ns = Declared(&[("p", "urn:p"), ("q", "urn:p")], Some("urn:default"));
    let t = ty("QName");
    let v = t.validate("p:a", &ns).expect("a QName");
    assert!(v.equals(&t.validate("q:a", &ns).expect("a QName")), "the prefix is not part of the value");
    assert!(!v.equals(&t.validate("p:b", &ns).expect("a QName")));
    assert_eq!(v.canonical(), "{urn:p}a");
    // No prefix: the default namespace.
    assert_eq!(t.validate("a", &ns).expect("a QName").canonical(), "{urn:default}a");
    assert_eq!(t.validate("a", &NoNamespaces).expect("a QName").canonical(), "a");
    // A prefix that is not declared is not valid.
    assert!(t.validate("x:a", &ns).is_err());
    assert!(t.validate("p:a", &NoNamespaces).is_err());
    for bad in ["", ":a", "a:", "a:b:c", "p:b:c", "1a", "a b", "p:1a"] {
        assert!(t.validate(bad, &ns).is_err(), "{bad:?}");
    }
    assert_eq!(t.validate("  p:a  ", &ns).expect("white space collapsed").canonical(), "{urn:p}a");
    let n = ty("NOTATION").validate("p:png", &ns).expect("a NOTATION");
    assert!(matches!(n, Value::Notation(_)));
    assert!(!n.equals(&v), "a NOTATION is not a QName");
}

#[test]
fn names() {
    check("Name", &["a", "a:b", "_x", ":x", "a-b.c", "é", "a\u{B7}", "x1", "日本"], &["1a", "-a", "a b", "", "a!", ".a", "\u{B7}a"]);
    check("NCName", &["a", "_x", "a-b.c", "x1"], &["a:b", ":x", "1a", "", "a b"]);
    check("NMTOKEN", &["abc", "1abc", "-a", "a:b", ".5"], &["a b", "", "a!", "<a>"]);
    for name in ["ID", "IDREF", "ENTITY"] {
        check(name, &["a", "_1", " x "], &["1", "a:b", "a b", ""]);
    }
    check(
        "language",
        &["en", "en-US", "x-klingon", "i-navajo", "e", "de-CH-1901", "zh-Hant-TW", "a-1", "abcdefgh", "en-abcdefgh"],
        &["en_US", "toolonglang", "", "en-", "-en", "e1", "en--US", "en-toolonglang", "abcdefghi", "en-abcdefghi"],
    );
    assert_eq!(value("NCName", " a ").canonical(), "a");
}

#[test]
fn lists() {
    for (name, yes, no) in [
        ("NMTOKENS", &["a", "a b c", " a  b ", "a\tb\nc"][..], &["", "   ", "a !"][..]),
        ("IDREFS", &["a b"], &["", "1 a", "a:b"]),
        ("ENTITIES", &["a"], &[""]),
    ] {
        check(name, yes, no);
    }
    let Value::List(items) = value("NMTOKENS", "  one   two\nthree ") else { panic!("a list") };
    assert_eq!(items.iter().map(Value::canonical).collect::<Vec<_>>(), ["one", "two", "three"]);
    assert_eq!(value("NMTOKENS", "a b").length(), Some(2));
    let e = ty("NMTOKENS").validate("a b !", &NoNamespaces).expect_err("the third item");
    assert!(e.message.contains("item 3"), "{e}");
    assert_eq!(kind("NMTOKENS", ""), ViolationKind::MinLength);
}

#[test]
fn integers() {
    check(
        "integer",
        &["0", "-0", "+5", "12345678901234567890123456789", "-007", " 5 ", "0000"],
        &["1.0", "", "1e3", "1.", ".5", "+", "--1", "1 2", "0x10", "١"],
    );
    assert!(value("integer", "+05").equals(&value("integer", "5")));
    assert_eq!(value("integer", "-000").canonical(), "0");
    assert_eq!(kind("integer", "1.5"), ViolationKind::Lexical, "an integer is defined by its lexical form");
    for (name, yes, no) in [
        ("nonPositiveInteger", &["0", "-5", "-0"][..], &["1", "+1"][..]),
        ("negativeInteger", &["-1", "-99999999999999999999"], &["0", "-0", "1"]),
        ("nonNegativeInteger", &["0", "-0", "5", "99999999999999999999"], &["-1"]),
        ("positiveInteger", &["1", "99999999999999999999"], &["0", "-0", "-1"]),
        ("long", &["-9223372036854775808", "9223372036854775807", "0"], &["-9223372036854775809", "9223372036854775808"]),
        ("int", &["-2147483648", "2147483647"], &["-2147483649", "2147483648"]),
        ("short", &["-32768", "32767"], &["-32769", "32768"]),
        ("byte", &["-128", "127", "0"], &["-129", "128"]),
        ("unsignedLong", &["0", "18446744073709551615"], &["-1", "18446744073709551616"]),
        ("unsignedInt", &["0", "4294967295"], &["-1", "4294967296"]),
        ("unsignedShort", &["0", "65535"], &["-1", "65536"]),
        ("unsignedByte", &["0", "255"], &["-1", "256"]),
    ] {
        check(name, yes, no);
    }
    assert_eq!(kind("unsignedByte", "256"), ViolationKind::MaxInclusive);
    assert_eq!(kind("unsignedByte", "-1"), ViolationKind::MinInclusive);
    assert_eq!(kind("positiveInteger", "0"), ViolationKind::MinInclusive);
}

#[test]
fn a_violation_has_the_name_of_the_constraint_and_says_what_is_wrong() {
    let e = ty("decimal").validate("abc", &NoNamespaces).expect_err("not a decimal");
    assert_eq!((e.kind, e.kind.code()), (ViolationKind::Lexical, "cvc-datatype-valid.1.2.1"));
    assert!(e.message.contains("'abc'") && e.message.contains("decimal"), "{e}");
    let e = ty("byte").validate("300", &NoNamespaces).expect_err("too big");
    assert_eq!(e.kind.code(), "cvc-maxInclusive-valid");
    assert!(e.message.contains("300") && e.message.contains("127"), "{e}");
}
