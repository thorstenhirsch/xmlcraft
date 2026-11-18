//! The canonical value of an Extended JSON text: what a JSON Schema is to be checked against.
//!
//! In Extended JSON a number is `{"$numberInt": "5"}` and an object id is `{"$oid": "…"}`; a schema written for the documents of a
//! collection talks about numbers and strings. [`bson_to_plain_json`] unwraps the type wrappers into the JSON value each stands for,
//! keeping the place in the text of every value (a wrapper's place is the place of the value it became), so that a problem found in the
//! plain value is shown at the right bytes of the Extended JSON text.
//!
//! | wrapper | becomes |
//! |---|---|
//! | `$numberInt`, `$numberLong`, `$numberDecimal` | the number (the digits as written) |
//! | `$numberDouble` | the number; `NaN`, `Infinity` and `-Infinity` stay as those words (as the YAML reader writes them) |
//! | `$oid`, `$uuid`, `$symbol`, `$code` | the string (`$code` with a `$scope` loses the scope) |
//! | `$date` | the ISO 8601 string (a `$numberLong` of milliseconds is written as one) |
//! | `$binary` | the base64 string |
//! | `$regularExpression` | the pattern |
//! | `$timestamp` | the object `{"t": …, "i": …}` |
//! | `$dbPointer` | the object `{"$ref": …, "$id": "…"}` |
//! | `$undefined`, `$minKey`, `$maxKey` | null |
//!
//! A wrapper that is not well formed is left as the object it is (the Extended JSON reader reports it, and a schema sees an object).

use xmlcraft_json_syntax::{Data, Member, Value};

/// The plain JSON value `value` (read from Extended JSON text) stands for.
pub fn bson_to_plain_json(value: &Value) -> Value {
    match &value.data {
        Data::Array(items) => Value { span: value.span.clone(), data: Data::Array(items.iter().map(bson_to_plain_json).collect()) },
        Data::Object(members) => match wrapper(value, members) {
            Some(plain) => plain,
            None => Value {
                span: value.span.clone(),
                data: Data::Object(
                    members
                        .iter()
                        .map(|m| Member { key: m.key.clone(), key_span: m.key_span.clone(), value: bson_to_plain_json(&m.value) })
                        .collect(),
                ),
            },
        },
        _ => value.clone(),
    }
}

fn text(value: &Value) -> Option<&str> {
    match &value.data {
        Data::String(s) => Some(s),
        _ => None,
    }
}

/// The member of `members` with exactly these keys (in any order), in the order of `keys`.
fn exact<'a>(members: &'a [Member], keys: &[&str]) -> Option<Vec<&'a Value>> {
    if members.len() != keys.len() {
        return None;
    }
    keys.iter().map(|k| members.iter().find(|m| m.key == *k).map(|m| &m.value)).collect()
}

fn number(value: &Value, digits: &str) -> Value {
    Value { span: value.span.clone(), data: Data::Number(digits.to_owned()) }
}

fn string(value: &Value, s: &str) -> Value {
    Value { span: value.span.clone(), data: Data::String(s.to_owned()) }
}

fn is_decimal_text(s: &str) -> bool {
    matches!(s, "NaN" | "Infinity" | "-Infinity")
        || s.parse::<f64>().is_ok_and(|_| s.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E')))
}

/// The plain value of the wrapper `value` (an object with `members`), if it is one that is well formed.
fn wrapper(value: &Value, members: &[Member]) -> Option<Value> {
    let key = if members.iter().any(|m| m.key == "$code") { "$code" } else { members.first()?.key.as_str() };
    let only = |k: &str| exact(members, &[k]).and_then(|v| v.first().copied());
    match key {
        "$oid" | "$symbol" | "$uuid" => Some(string(value, text(only(key)?)?)),
        "$numberInt" => {
            let s = text(only(key)?)?;
            s.parse::<i32>().ok().map(|_| number(value, s))
        }
        "$numberLong" => {
            let s = text(only(key)?)?;
            s.parse::<i64>().ok().map(|_| number(value, s))
        }
        "$numberDouble" | "$numberDecimal" => {
            let s = text(only(key)?)?;
            is_decimal_text(s).then(|| number(value, s))
        }
        "$date" => {
            let inner = only(key)?;
            match &inner.data {
                Data::String(s) => Some(string(value, s)),
                Data::Object(parts) => {
                    let ms: i64 = text(exact(parts, &["$numberLong"])?.first().copied()?)?.parse().ok()?;
                    let date = chrono::DateTime::from_timestamp_millis(ms)?;
                    Some(string(value, &date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)))
                }
                _ => None,
            }
        }
        "$binary" => {
            let Data::Object(parts) = &only(key)?.data else { return None };
            let parts = exact(parts, &["base64", "subType"])?;
            Some(string(value, text(parts.first().copied()?)?))
        }
        "$regularExpression" => {
            let Data::Object(parts) = &only(key)?.data else { return None };
            let parts = exact(parts, &["pattern", "options"])?;
            Some(string(value, text(parts.first().copied()?)?))
        }
        "$timestamp" => {
            let inner = only(key)?;
            let Data::Object(parts) = &inner.data else { return None };
            exact(parts, &["t", "i"])?;
            Some(Value {
                span: value.span.clone(),
                data: Data::Object(
                    parts.iter().map(|m| Member { key: m.key.clone(), key_span: m.key_span.clone(), value: m.value.clone() }).collect(),
                ),
            })
        }
        "$undefined" | "$minKey" | "$maxKey" => {
            only(key)?;
            Some(Value { span: value.span.clone(), data: Data::Null })
        }
        "$code" => {
            if members.len() == 1 {
                return Some(string(value, text(only(key)?)?));
            }
            let parts = exact(members, &["$code", "$scope"])?;
            Some(string(value, text(parts.first().copied()?)?))
        }
        "$dbPointer" => {
            let Data::Object(parts) = &only(key)?.data else { return None };
            let parts = exact(parts, &["$ref", "$id"])?;
            let namespace = text(parts.first().copied()?)?;
            let id = parts.get(1).copied()?;
            let Data::Object(id_members) = &id.data else { return None };
            let oid = text(exact(id_members, &["$oid"])?.first().copied()?)?;
            Some(Value {
                span: value.span.clone(),
                data: Data::Object(vec![
                    Member { key: "$ref".to_owned(), key_span: value.span.clone(), value: string(value, namespace) },
                    Member { key: "$id".to_owned(), key_span: value.span.clone(), value: string(value, oid) },
                ]),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use xmlcraft_json_syntax::{Options, parse};

    use super::*;

    /// The plain value of the Extended JSON `text`, as compact JSON text.
    fn plain(text: &str) -> String {
        let read = parse(text, &Options::default());
        let root = read.root.expect("a value");
        show(&bson_to_plain_json(&root))
    }

    fn show(value: &Value) -> String {
        match &value.data {
            Data::Null => "null".to_owned(),
            Data::Bool(b) => b.to_string(),
            Data::Number(n) => n.clone(),
            Data::String(s) => format!("{s:?}"),
            Data::Array(items) => format!("[{}]", items.iter().map(show).collect::<Vec<_>>().join(",")),
            Data::Object(members) => {
                format!("{{{}}}", members.iter().map(|m| format!("{:?}:{}", m.key, show(&m.value))).collect::<Vec<_>>().join(","))
            }
        }
    }

    #[test]
    fn numbers_are_numbers_and_the_special_doubles_keep_their_words() {
        assert_eq!(plain(r#"{"$numberInt": "5"}"#), "5");
        assert_eq!(plain(r#"{"$numberLong": "-9223372036854775808"}"#), "-9223372036854775808");
        assert_eq!(plain(r#"{"$numberDouble": "1.5"}"#), "1.5");
        assert_eq!(plain(r#"[{"$numberDouble": "NaN"}, {"$numberDouble": "Infinity"}, {"$numberDouble": "-Infinity"}]"#), "[NaN,Infinity,-Infinity]");
        assert_eq!(plain(r#"{"$numberDecimal": "1.0E+3"}"#), "1.0E+3");
        assert_eq!(plain(r#"{"$numberDecimal": "NaN"}"#), "NaN");
    }

    #[test]
    fn identifiers_dates_binary_data_expressions_and_timestamps_become_what_a_schema_can_talk_about() {
        assert_eq!(plain(r#"{"$oid": "507f1f77bcf86cd799439011"}"#), "\"507f1f77bcf86cd799439011\"");
        assert_eq!(plain(r#"{"$date": "2024-03-01T10:00:00Z"}"#), "\"2024-03-01T10:00:00Z\"");
        assert_eq!(plain(r#"{"$date": {"$numberLong": "1709287200000"}}"#), "\"2024-03-01T10:00:00.000Z\"");
        assert_eq!(plain(r#"{"$binary": {"base64": "AQID", "subType": "00"}}"#), "\"AQID\"");
        assert_eq!(plain(r#"{"$uuid": "73ffd264-44b3-4c69-90e8-e7d1dfc035d4"}"#), "\"73ffd264-44b3-4c69-90e8-e7d1dfc035d4\"");
        assert_eq!(plain(r#"{"$regularExpression": {"pattern": "^a.*", "options": "i"}}"#), "\"^a.*\"");
        assert_eq!(plain(r#"{"$timestamp": {"t": 1, "i": 2}}"#), "{\"t\":1,\"i\":2}");
        assert_eq!(plain(r#"[{"$undefined": true}, {"$minKey": 1}, {"$maxKey": 1}]"#), "[null,null,null]");
        assert_eq!(plain(r#"{"$symbol": "s"}"#), "\"s\"");
        assert_eq!(plain(r#"{"$code": "f()"}"#), "\"f()\"");
        assert_eq!(plain(r#"{"$code": "f()", "$scope": {"x": 1}}"#), "\"f()\"");
        assert_eq!(
            plain(r#"{"$dbPointer": {"$ref": "c", "$id": {"$oid": "507f1f77bcf86cd799439011"}}}"#),
            "{\"$ref\":\"c\",\"$id\":\"507f1f77bcf86cd799439011\"}"
        );
    }

    #[test]
    fn wrappers_inside_documents_and_arrays_are_unwrapped_and_everything_else_stays() {
        assert_eq!(
            plain(
                r#"{"_id": {"$oid": "507f1f77bcf86cd799439011"}, "n": {"$numberInt": "3"}, "tags": ["a", {"$numberLong": "4"}], "ok": true, "o": {"deep": {"$numberInt": "1"}}}"#
            ),
            "{\"_id\":\"507f1f77bcf86cd799439011\",\"n\":3,\"tags\":[\"a\",4],\"ok\":true,\"o\":{\"deep\":1}}"
        );
    }

    #[test]
    fn a_wrapper_that_is_not_well_formed_stays_an_object() {
        assert_eq!(plain(r#"{"$numberInt": "five"}"#), "{\"$numberInt\":\"five\"}");
        assert_eq!(plain(r#"{"$numberInt": "5", "x": 1}"#), "{\"$numberInt\":\"5\",\"x\":1}");
        assert_eq!(plain(r#"{"$date": 12}"#), "{\"$date\":12}");
        assert_eq!(plain(r#"{"$oid": 1}"#), "{\"$oid\":1}");
        assert_eq!(plain("{\"$unknown\": 1}"), "{\"$unknown\":1}");
        assert_eq!(plain("{}"), "{}");
    }

    #[test]
    fn a_wrapper_has_the_place_of_the_value_it_became() {
        let text = r#"{"n": {"$numberInt": "5"}}"#;
        let root = parse(text, &Options::default()).root.expect("a value");
        let plain = bson_to_plain_json(&root);
        let place = plain.span_of_pointer("/n").expect("a place");
        assert_eq!(&text[place], "\"n\"", "the name of the member is the place of a problem with it");
        assert_eq!(plain.pointer("/n").map(|v| &text[v.span.clone()]), Some(r#"{"$numberInt": "5"}"#));
    }
}
