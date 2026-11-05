//! BSON bytes and Extended JSON text: what is read is written again, and nothing hostile gets past the bounds.

use proptest::prelude::*;
use xmlcraft_bson::{Bson, Document, Layout, MAX_DEPTH, from_text, read_all, to_text, write_all};

fn doc(fields: Vec<(&str, Bson)>) -> Document {
    Document { fields: fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect() }
}

#[test]
fn the_examples_of_the_specification_are_read_and_written_as_they_are() {
    let hello: &[u8] = b"\x16\0\0\0\x02hello\0\x06\0\0\0world\0\0";
    let docs = read_all(hello).expect("reads");
    assert_eq!(docs, [doc(vec![("hello", Bson::String("world".into()))])]);
    assert_eq!(write_all(&docs), hello);
    let awesome: Vec<u8> = (0..49)
        .map(|i| {
            u8::from_str_radix(
                &"310000000442534f4e002600000002300008000000617765736f6d65000131003333333333331440103200c20700000000"[i * 2..i * 2 + 2],
                16,
            )
            .expect("hex")
        })
        .collect();
    let docs = read_all(&awesome).expect("reads");
    assert_eq!(docs, [doc(vec![("BSON", Bson::Array(vec![Bson::String("awesome".into()), Bson::Double(5.05), Bson::Int32(1986)]))])]);
    assert_eq!(write_all(&docs), awesome);
    assert_eq!(read_all(b""), Ok(Vec::new()));
}

fn every_type() -> Document {
    doc(vec![
        ("double", Bson::Double(1.5)),
        ("nan", Bson::Double(f64::NAN)),
        ("inf", Bson::Double(f64::NEG_INFINITY)),
        ("string", Bson::String("héllo \"q\" \n\u{0}".into())),
        ("doc", Bson::Document(doc(vec![("a", Bson::Null)]))),
        ("array", Bson::Array(vec![Bson::Int32(1), Bson::Array(vec![]), Bson::Document(Document::default())])),
        ("binary", Bson::Binary { subtype: 0x80, bytes: vec![0, 1, 2, 255] }),
        ("uuid", Bson::Binary { subtype: 4, bytes: (0..16).collect() }),
        ("undefined", Bson::Undefined),
        ("oid", Bson::ObjectId([0xAB; 12])),
        ("bool", Bson::Bool(true)),
        ("date", Bson::DateTime(1_700_000_000_123)),
        ("old date", Bson::DateTime(-5_000_000_000_000)),
        ("null", Bson::Null),
        ("regex", Bson::Regex { pattern: "^a.*".into(), options: "i".into() }),
        ("dbpointer", Bson::DbPointer { namespace: "db.coll".into(), id: [1; 12] }),
        ("code", Bson::JavaScript("function(){}".into())),
        ("symbol", Bson::Symbol("sym".into())),
        ("scoped", Bson::JavaScriptWithScope { code: "x".into(), scope: doc(vec![("x", Bson::Int32(1))]) }),
        ("int32", Bson::Int32(-7)),
        ("timestamp", Bson::Timestamp { time: 4_000_000_000, increment: 9 }),
        ("int64", Bson::Int64(i64::MIN)),
        ("decimal", Bson::Decimal128(xmlcraft_bson_decimal("12.50"))),
        ("min", Bson::MinKey),
        ("max", Bson::MaxKey),
    ])
}

/// A decimal128 by its text, through the text projection (the module is not public).
fn xmlcraft_bson_decimal(text: &str) -> [u8; 16] {
    let json = format!("{{\"d\": {{\"$numberDecimal\": \"{text}\"}}}}");
    match from_text(&json).expect("a decimal").remove(0).fields.remove(0).1 {
        Bson::Decimal128(d) => d,
        other => panic!("{other:?}"),
    }
}

#[test]
fn every_type_survives_bytes_and_canonical_text_exactly() {
    let docs = vec![every_type()];
    let bytes = write_all(&docs);
    let read = read_all(&bytes).expect("reads");
    assert_eq!(write_all(&read), bytes, "bytes to model to bytes");
    let (text, layout) = to_text(&read, true);
    assert_eq!(layout, Layout::Single);
    let back = from_text(&text).unwrap_or_else(|e| panic!("{e:?} in {text}"));
    assert_eq!(write_all(&back), bytes, "bytes to text to bytes: {text}");
    assert!(
        text.contains("\"$numberInt\": \"-7\"")
            && text.contains("\"$numberDecimal\": \"12.50\"")
            && text.contains("\"$oid\": \"abababababababababababab\""),
        "{text}"
    );
    assert!(text.contains("\"$binary\": {\"base64\": \"AAEC/w==\", \"subType\": \"80\"}"), "{text}");
}

#[test]
fn relaxed_text_has_plain_numbers_and_dates_and_still_reads_back() {
    let docs = vec![doc(vec![
        ("n", Bson::Int32(5)),
        ("l", Bson::Int64(1 << 40)),
        ("f", Bson::Double(2.5)),
        ("d", Bson::DateTime(1_700_000_000_123)),
        ("big", Bson::Int64(i64::MAX)),
    ])];
    let (text, _) = to_text(&docs, false);
    assert!(text.contains("\"n\": 5") && text.contains("\"f\": 2.5") && text.contains("\"$date\": \"2023-11-14T22:13:20.123Z\""), "{text}");
    let back = from_text(&text).expect("reads");
    // Plain integers become int32 when they fit and int64 when they do not: a relaxed text does not say which it was.
    assert_eq!(back[0].fields[0].1, Bson::Int32(5));
    assert_eq!(back[0].fields[1].1, Bson::Int64(1 << 40));
    assert_eq!(back[0].fields[3].1, Bson::DateTime(1_700_000_000_123));
}

#[test]
fn a_date_outside_the_years_of_the_relaxed_form_is_spelled_out() {
    let docs = vec![doc(vec![
        ("a", Bson::DateTime(-1)),
        ("b", Bson::DateTime(253_402_300_800_000)),
        ("c", Bson::DateTime(0)),
        ("d", Bson::DateTime(253_402_300_799_999)),
    ])];
    let (text, _) = to_text(&docs, false);
    assert!(text.contains("\"a\": {\"$date\": {\"$numberLong\": \"-1\"}}"), "{text}");
    assert!(text.contains("\"b\": {\"$date\": {\"$numberLong\": \"253402300800000\"}}"), "year 10000: {text}");
    assert!(text.contains("\"c\": {\"$date\": \"1970-01-01T00:00:00.000Z\"}") && text.contains("9999-12-31T23:59:59.999Z"), "{text}");
    assert_eq!(write_all(&from_text(&text).expect("reads")), write_all(&docs));
}

#[test]
fn several_documents_are_one_to_a_line_and_one_may_be_indented() {
    let docs = vec![doc(vec![("a", Bson::Int32(1))]), doc(vec![("b", Bson::String("x".into()))])];
    let (text, layout) = to_text(&docs, true);
    assert_eq!((layout, text.lines().count()), (Layout::Lines, 2));
    assert_eq!(from_text(&text).expect("reads"), docs);
    let (single, layout) = to_text(&docs[..1], true);
    assert_eq!(layout, Layout::Single);
    assert!(single.lines().count() > 1 && single.starts_with("{\n  \"a\": {\"$numberInt\": \"1\"}\n}"), "{single}");
    assert_eq!(from_text(&single).expect("reads"), docs[..1]);
    assert_eq!(from_text(""), Ok(Vec::new()));
    assert_eq!(from_text("\n  \n"), Ok(Vec::new()));
    assert_eq!(to_text(&[], true).0, "");
}

#[test]
fn text_that_is_not_extended_json_is_reported_where_it_is() {
    let text = "{\"a\": 1, \"id\": {\"$oid\": \"zz\"}}";
    let errors = from_text(text).expect_err("bad");
    assert_eq!(errors.len(), 1);
    assert_eq!(&text[errors[0].offset..errors[0].offset + errors[0].length], "\"zz\"");
    assert!(errors[0].message.contains("24 hexadecimal"));
    for (bad, expected) in [
        ("[1]", "an object"),
        ("{\"a\": {\"$numberInt\": \"x\"}}", "32-bit"),
        ("{\"a\": {\"$date\": \"yesterday\"}}", "ISO 8601"),
        ("{\"a\": {\"$binary\": {\"base64\": \"!\", \"subType\": \"00\"}}}", "base64"),
        ("{\"a\": {\"$timestamp\": {\"t\": 1}}}", "t and i"),
        ("{\"a\": {\"$numberDecimal\": \"1.2.3\"}}", "not a decimal"),
        ("{\"a\": 1e999}", "number"),
    ] {
        let errors = from_text(bad).expect_err(bad);
        assert!(errors[0].message.contains(expected), "{bad}: {:?}", errors[0]);
    }
    // Syntax errors of the JSON are errors too; a `$` field with no meaning is an ordinary field.
    assert!(from_text("{\"a\": ").is_err());
    assert_eq!(from_text("{\"$custom\": 1}").expect("ordinary").len(), 1);
}

#[test]
fn damaged_bytes_are_errors_that_say_where_and_never_panics() {
    let good = write_all(&[every_type()]);
    // Every prefix and every single-byte damage.
    for len in 0..good.len() {
        let _ = read_all(&good[..len]);
    }
    for at in 0..good.len() {
        for byte in [0u8, 1, 0x7F, 0xFF] {
            let mut damaged = good.clone();
            damaged[at] = byte;
            let _ = read_all(&damaged);
        }
    }
    for (bytes, expected) in [
        (&b"\x05\0\0\0"[..], "longer"),
        (b"\x04\0\0\0\0", "below five"),
        (b"\xFF\xFF\xFF\x7F\0", "longer"),
        (b"\x0E\0\0\0\x02a\0\x02\0\0\0\xFF\0\0", "UTF-8"),
        (b"\x08\0\0\0\x20a\0\0", "not a BSON type"),
        (b"\x09\0\0\0\x08a\0\x02\0", "neither 0 nor 1"),
        // Code with a scope whose own length is one too many.
        (b"\x17\0\0\0\x0Fa\0\x10\0\0\0\x02\0\0\0x\0\x05\0\0\0\0\0", "does not fit"),
        (b"\x10\0\0\0\x04a\0\x08\0\0\0\x0Ax\0\0\0", "an array has the name"),
    ] {
        let e = read_all(bytes).expect_err(&format!("{bytes:?}"));
        assert!(e.message.contains(expected), "{bytes:?}: {e}");
    }
    // Nesting is bounded.
    let mut nested = vec![Bson::Null];
    for _ in 0..MAX_DEPTH + 5 {
        nested = vec![Bson::Array(nested)];
    }
    let deep = write_all(&[doc(vec![("a", Bson::Array(nested))])]);
    assert!(read_all(&deep).expect_err("too deep").message.contains("nested deeper"));
}

fn name() -> impl Strategy<Value = String> {
    "[a-z$.0-9é ]{0,6}"
}

fn arb_bson() -> impl Strategy<Value = Bson> {
    let leaf = prop_oneof![
        any::<f64>().prop_map(|f| Bson::Double(if f.is_nan() { f64::NAN } else { f })),
        "[^\u{0}]{0,10}".prop_map(Bson::String),
        (any::<u8>(), prop::collection::vec(any::<u8>(), 0..8)).prop_map(|(subtype, bytes)| Bson::Binary { subtype, bytes }),
        Just(Bson::Undefined),
        any::<[u8; 12]>().prop_map(Bson::ObjectId),
        any::<bool>().prop_map(Bson::Bool),
        any::<i64>().prop_map(Bson::DateTime),
        Just(Bson::Null),
        ("[^\u{0}]{0,6}", "[a-z]{0,3}").prop_map(|(pattern, options)| Bson::Regex { pattern, options }),
        ("[^\u{0}]{0,6}", any::<[u8; 12]>()).prop_map(|(namespace, id)| Bson::DbPointer { namespace, id }),
        ".{0,6}".prop_map(Bson::JavaScript),
        ".{0,6}".prop_map(Bson::Symbol),
        any::<i32>().prop_map(Bson::Int32),
        (any::<u32>(), any::<u32>()).prop_map(|(time, increment)| Bson::Timestamp { time, increment }),
        any::<i64>().prop_map(Bson::Int64),
        (any::<i64>(), -20i32..20).prop_map(|(m, e)| Bson::Decimal128(xmlcraft_bson_decimal(&format!("{m}E{e}")))),
        Just(Bson::MinKey),
        Just(Bson::MaxKey),
    ];
    leaf.prop_recursive(3, 30, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Bson::Array),
            prop::collection::vec((name(), inner.clone()), 0..4).prop_map(|f| Bson::Document(Document { fields: f })),
            (".{0,4}", prop::collection::vec((name(), inner), 0..3))
                .prop_map(|(code, f)| Bson::JavaScriptWithScope { code, scope: Document { fields: f } }),
        ]
    })
}

proptest! {
    #[test]
    fn any_documents_survive_bytes_and_canonical_text(fields in prop::collection::vec((name(), arb_bson()), 0..5), more in 0usize..3) {
        let docs: Vec<Document> = (0..=more).map(|_| Document { fields: fields.clone() }).collect();
        let bytes = write_all(&docs);
        let read = read_all(&bytes).map_err(|e| TestCaseError::fail(format!("{e}")))?;
        prop_assert_eq!(write_all(&read), bytes.clone());
        let (text, _) = to_text(&read, true);
        let back = from_text(&text).map_err(|e| TestCaseError::fail(format!("{e:?} in {text}")))?;
        prop_assert_eq!(write_all(&back), bytes, "text was {}", text);
        // The relaxed text always reads (types may differ, nothing is invalid).
        let (relaxed, _) = to_text(&read, false);
        prop_assert!(from_text(&relaxed).is_ok(), "{}", relaxed);
    }

    #[test]
    fn random_bytes_and_random_text_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..200), text in ".{0,100}") {
        let _ = read_all(&bytes);
        let _ = from_text(&text);
    }

    #[test]
    fn damaged_valid_bytes_never_panic(fields in prop::collection::vec((name(), arb_bson()), 0..4), at in 0usize..400, byte in any::<u8>()) {
        let mut bytes = write_all(&[Document { fields }]);
        if let Some(slot) = bytes.get_mut(at) {
            *slot = byte;
        }
        let _ = read_all(&bytes);
    }
}
