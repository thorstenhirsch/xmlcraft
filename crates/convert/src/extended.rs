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
    match &value.data {
        Data::Array(items) => Value { span: 0..0, data: Data::Array(items.iter().map(extended_to_plain).collect()) },
        Data::Object(members) => typed(members).unwrap_or_else(|| Value {
            span: 0..0,
            data: Data::Object(members.iter().map(|m| Member { key: m.key.clone(), key_span: 0..0, value: extended_to_plain(&m.value) }).collect()),
        }),
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
            if s.parse::<f64>().is_ok_and(f64::is_finite) && s.len() <= 17 { plain_number(s) } else { string(s) }
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
        "$undefined" => Value { span: 0..0, data: Data::Null },
        "$minKey" | "$maxKey" => string(first),
        "$dbPointer" => match &only(members, first)?.data {
            Data::Object(inner) => extended_to_plain(&Value { span: 0..0, data: Data::Object(inner.clone()) }),
            _ => return None,
        },
        _ => return None,
    })
}
