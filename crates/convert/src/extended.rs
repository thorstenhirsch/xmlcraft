//! MongoDB Extended JSON values turned into the plain values other formats have.

use xmlcraft_json_syntax::{Data, Member, Value};

fn text(v: &Value) -> Option<&str> {
    if let Data::String(s) = &v.data { Some(s) } else { None }
}

fn plain_number(n: &str) -> Value {
    Value { span: 0..0, data: Data::Number(n.to_owned()) }
}

fn string(s: impl Into<String>) -> Value {
    Value { span: 0..0, data: Data::String(s.into()) }
}

fn only<'a>(members: &'a [Member], key: &str) -> Option<&'a Value> {
    (members.len() == 1).then(|| members.first().filter(|m| m.key == key).map(|m| &m.value)).flatten()
}

/// `value` with every typed Extended JSON object replaced by a plain one: numbers by numbers (a `$numberDecimal` that is finite, a
/// `$numberDouble` that is), `$oid`, `$uuid`, `$symbol`, `$code`, `$minKey`/`$maxKey` and dates by strings (an ISO date where the
/// years allow, else the milliseconds), `$binary` by its base64 text, `$regularExpression` by `/pattern/options`, `$timestamp` by an
/// object, `$undefined` by null. Other objects keep their members.
pub fn extended_to_plain(value: &Value) -> Value {
    extended_to_plain_noting(value).0
}

/// [`extended_to_plain`], and whether a value was replaced by one that cannot say what it was (not a number).
pub(crate) fn extended_to_plain_noting(value: &Value) -> (Value, bool) {
    let mut changed = false;
    let out = plain(value, &mut changed);
    (out, changed)
}

fn plain(value: &Value, changed: &mut bool) -> Value {
    match &value.data {
        Data::Array(items) => Value { span: 0..0, data: Data::Array(items.iter().map(|v| plain(v, changed)).collect()) },
        Data::Object(members) => match typed(members) {
            Some(v) => {
                // (A number type is a number in the other formats too; any other type, a date before 1970 that is only the milliseconds
                // now included, is what is lost.)
                let number_type = members.first().is_some_and(|m| m.key.starts_with("$number"));
                *changed |= !(number_type && matches!(v.data, Data::Number(_)));
                v
            }
            None => Value {
                span: 0..0,
                data: Data::Object(members.iter().map(|m| Member { key: m.key.clone(), key_span: 0..0, value: plain(&m.value, changed) }).collect()),
            },
        },
        other => Value { span: 0..0, data: other.clone() },
    }
}

fn typed(members: &[Member]) -> Option<Value> {
    let first = members.first()?.key.as_str();
    Some(match first {
        "$oid" | "$symbol" | "$code" if members.len() == 1 => string(text(only(members, first)?)?),
        "$uuid" => string(text(only(members, first)?)?),
        "$numberInt" | "$numberLong" => plain_number(text(only(members, first)?)?),
        "$numberDouble" => {
            let s = text(only(members, first)?)?;
            if s.parse::<f64>().is_ok_and(f64::is_finite) { plain_number(s) } else { string(s) }
        }
        "$numberDecimal" => {
            let s = text(only(members, first)?)?;
            if s.parse::<f64>().is_ok_and(f64::is_finite) { plain_number(s) } else { string(s) }
        }
        "$date" => match &only(members, first)?.data {
            Data::String(s) => string(s.clone()),
            Data::Object(inner) => {
                let ms: i64 = text(only(inner, "$numberLong")?)?.parse().ok()?;
                use chrono::{Datelike, SecondsFormat};
                match chrono::DateTime::from_timestamp_millis(ms).filter(|d| (1970..=9999).contains(&d.year())) {
                    Some(date) => string(date.to_rfc3339_opts(SecondsFormat::Millis, true)),
                    None => plain_number(&ms.to_string()),
                }
            }
            _ => return None,
        },
        "$binary" => match &only(members, first)?.data {
            Data::Object(inner) => string(text(&inner.iter().find(|m| m.key == "base64")?.value)?),
            _ => return None,
        },
        "$regularExpression" => match &only(members, first)?.data {
            Data::Object(inner) => {
                let get = |k: &str| inner.iter().find(|m| m.key == k).and_then(|m| text(&m.value));
                string(format!("/{}/{}", get("pattern")?, get("options")?))
            }
            _ => return None,
        },
        "$timestamp" => match &only(members, first)?.data {
            Data::Object(inner) => Value {
                span: 0..0,
                data: Data::Object(
                    inner
                        .iter()
                        .map(|m| Member { key: m.key.clone(), key_span: 0..0, value: Value { span: 0..0, data: m.value.data.clone() } })
                        .collect(),
                ),
            },
            _ => return None,
        },
        // (Only the object that is nothing but this member stands for the value; another keeps its members.)
        "$undefined" => {
            only(members, first)?;
            Value { span: 0..0, data: Data::Null }
        }
        "$minKey" | "$maxKey" => {
            only(members, first)?;
            string(first)
        }
        "$dbPointer" => match &only(members, first)?.data {
            Data::Object(inner) => plain(&Value { span: 0..0, data: Data::Object(inner.clone()) }, &mut false),
            _ => return None,
        },
        _ => return None,
    })
}
