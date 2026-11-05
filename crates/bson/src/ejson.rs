//! MongoDB Extended JSON v2, the text a BSON file is shown as: canonical (every type is spelled out, so the bytes come back exactly) or
//! relaxed (plain numbers and ISO dates where that loses nothing visible).

use xmlcraft_json_syntax::{Data, Dialect, Options, Severity, Value, parse, parse_lines};

use crate::base64;
use crate::decimal;
use crate::model::{Bson, BsonError, Document, binary_problem, depth};

/// A JSON value being built for output.
enum J {
    Null,
    Bool(bool),
    Str(String),
    Num(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

fn wrap(key: &str, value: J) -> J {
    J::Obj(vec![(key.to_owned(), value)])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn fmt_double(f: f64) -> String {
    if f.is_nan() {
        "NaN".to_owned()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else {
        format!("{f:?}")
    }
}

fn to_j(value: &Bson, canonical: bool) -> J {
    match value {
        Bson::Double(f) if canonical || !f.is_finite() => wrap("$numberDouble", J::Str(fmt_double(*f))),
        Bson::Double(f) => J::Num(fmt_double(*f)),
        Bson::String(s) => J::Str(s.clone()),
        Bson::Document(d) => doc_j(d, canonical),
        Bson::Array(items) => J::Arr(items.iter().map(|v| to_j(v, canonical)).collect()),
        Bson::Binary { subtype, bytes } => wrap(
            "$binary",
            J::Obj(vec![("base64".to_owned(), J::Str(base64::encode(bytes))), ("subType".to_owned(), J::Str(format!("{subtype:02x}")))]),
        ),
        Bson::Undefined => wrap("$undefined", J::Bool(true)),
        Bson::ObjectId(id) => wrap("$oid", J::Str(hex(id))),
        Bson::Bool(b) => J::Bool(*b),
        Bson::DateTime(ms) => {
            let relaxed = (!canonical).then(|| iso(*ms)).flatten();
            match relaxed {
                Some(text) => wrap("$date", J::Str(text)),
                None => wrap("$date", wrap("$numberLong", J::Str(ms.to_string()))),
            }
        }
        Bson::Null => J::Null,
        Bson::Regex { pattern, options } => {
            wrap("$regularExpression", J::Obj(vec![("pattern".to_owned(), J::Str(pattern.clone())), ("options".to_owned(), J::Str(options.clone()))]))
        }
        Bson::DbPointer { namespace, id } => {
            wrap("$dbPointer", J::Obj(vec![("$ref".to_owned(), J::Str(namespace.clone())), ("$id".to_owned(), wrap("$oid", J::Str(hex(id))))]))
        }
        Bson::JavaScript(code) => wrap("$code", J::Str(code.clone())),
        Bson::Symbol(s) => wrap("$symbol", J::Str(s.clone())),
        Bson::JavaScriptWithScope { code, scope } => {
            J::Obj(vec![("$code".to_owned(), J::Str(code.clone())), ("$scope".to_owned(), doc_j(scope, canonical))])
        }
        Bson::Int32(n) if canonical => wrap("$numberInt", J::Str(n.to_string())),
        Bson::Int32(n) => J::Num(n.to_string()),
        Bson::Timestamp { time, increment } => {
            wrap("$timestamp", J::Obj(vec![("t".to_owned(), J::Num(time.to_string())), ("i".to_owned(), J::Num(increment.to_string()))]))
        }
        Bson::Int64(n) if canonical => wrap("$numberLong", J::Str(n.to_string())),
        Bson::Int64(n) => J::Num(n.to_string()),
        Bson::Decimal128(d) => wrap("$numberDecimal", J::Str(decimal::to_string(d))),
        Bson::MinKey => wrap("$minKey", J::Num("1".to_owned())),
        Bson::MaxKey => wrap("$maxKey", J::Num("1".to_owned())),
    }
}

fn doc_j(doc: &Document, canonical: bool) -> J {
    J::Obj(doc.fields.iter().map(|(k, v)| (k.clone(), to_j(v, canonical))).collect())
}

/// The ISO 8601 text of a date in the years the relaxed form uses (1970 to 9999), else none.
fn iso(ms: i64) -> Option<String> {
    use chrono::{Datelike, SecondsFormat};
    let date = chrono::DateTime::from_timestamp_millis(ms)?;
    (1970..=9999).contains(&date.year()).then(|| date.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn compact(j: &J, out: &mut String) {
    match j {
        J::Null => out.push_str("null"),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Str(s) => quote(s, out),
        J::Num(n) => out.push_str(n),
        J::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                compact(item, out);
            }
            out.push(']');
        }
        J::Obj(members) => {
            out.push('{');
            for (i, (k, v)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                quote(k, out);
                out.push(':');
                compact(v, out);
            }
            out.push('}');
        }
    }
}

/// A wrapper such as `{"$oid": "…"}` is kept on one line when it is short.
fn is_inline(j: &J) -> bool {
    match j {
        J::Obj(members) => {
            members.len() <= 2
                && members.first().is_some_and(|(k, _)| k.starts_with('$'))
                && members.iter().all(|(_, v)| !matches!(v, J::Arr(_)) && is_flat(v))
        }
        _ => false,
    }
}

fn is_flat(j: &J) -> bool {
    match j {
        J::Arr(_) => false,
        J::Obj(members) => members.iter().all(|(_, v)| is_flat(v)),
        _ => true,
    }
}

/// On one line, with a space after each `:` and `,`.
fn inline(j: &J, out: &mut String) {
    match j {
        J::Obj(members) => {
            out.push('{');
            for (i, (k, v)) in members.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                quote(k, out);
                out.push_str(": ");
                inline(v, out);
            }
            out.push('}');
        }
        other => compact(other, out),
    }
}

fn pretty(j: &J, level: usize, out: &mut String) {
    let pad = |n: usize| "  ".repeat(n);
    match j {
        J::Arr(items) if !items.is_empty() => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&pad(level + 1));
                pretty(item, level + 1, out);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(level));
            out.push(']');
        }
        J::Obj(members) if !members.is_empty() && !is_inline(j) => {
            out.push_str("{\n");
            for (i, (k, v)) in members.iter().enumerate() {
                out.push_str(&pad(level + 1));
                quote(k, out);
                out.push_str(": ");
                pretty(v, level + 1, out);
                out.push_str(if i + 1 < members.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(level));
            out.push('}');
        }
        J::Obj(_) => inline(j, out),
        other => compact(other, out),
    }
}

/// How the documents of a file are laid out as text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One document, indented over lines.
    Single,
    /// One document on each line.
    Lines,
}

/// The text of `documents`: a single document is indented; several are one to a line (so a dump of a million is still a text with a
/// million lines). `canonical` spells out every type, which is what makes the text turn back into the same bytes.
pub fn to_text(documents: &[Document], canonical: bool) -> Result<(String, Layout), BsonError> {
    for document in documents {
        if let Some(name) = ambiguous(document) {
            return Err(BsonError {
                offset: 0,
                length: 0,
                message: format!("a field named '{name}' cannot be shown in Extended JSON: it would read back as a value of another type"),
            });
        }
    }
    let mut out = String::new();
    if let [only] = documents {
        pretty(&doc_j(only, canonical), 0, &mut out);
        out.push('\n');
        return Ok((out, Layout::Single));
    }
    for document in documents {
        compact(&doc_j(document, canonical), &mut out);
        out.push('\n');
    }
    Ok((out, Layout::Lines))
}

const WRAPPERS: &[&str] = &[
    "$oid",
    "$numberInt",
    "$numberLong",
    "$numberDouble",
    "$numberDecimal",
    "$date",
    "$binary",
    "$uuid",
    "$regularExpression",
    "$timestamp",
    "$undefined",
    "$minKey",
    "$maxKey",
    "$symbol",
    "$code",
    "$dbPointer",
];

/// The name of a field that would make the text read back as a value of another type: the first field of a document that is a wrapper's
/// name, or a `$code` field anywhere.
fn ambiguous(doc: &Document) -> Option<&str> {
    let first = doc.fields.first().map(|(k, _)| k.as_str()).filter(|k| WRAPPERS.contains(k));
    let code = doc.fields.iter().map(|(k, _)| k.as_str()).find(|k| *k == "$code");
    if let Some(name) = first.or(code) {
        return Some(name);
    }
    fn inside(value: &Bson) -> Option<&str> {
        match value {
            Bson::Document(d) => ambiguous(d),
            Bson::Array(items) => items.iter().find_map(inside),
            Bson::JavaScriptWithScope { scope, .. } => ambiguous(scope),
            _ => None,
        }
    }
    doc.fields.iter().find_map(|(_, v)| inside(v))
}

fn problem(value: &Value, message: impl Into<String>) -> BsonError {
    BsonError { offset: value.span.start, length: value.span.len(), message: message.into() }
}

/// The documents a text stands for, or every problem found: syntax errors of the JSON, and values that are not valid Extended JSON
/// (with where they are in the text). One document may be spread over lines; several are one to a line.
pub fn from_text(text: &str) -> Result<Vec<Document>, Vec<BsonError>> {
    // (A wrapper is an object of its own: the text may nest about twice as deep as the document it stands for.)
    let options = Options { dialect: Dialect::Json, max_depth: crate::MAX_DEPTH * 2 + 4, ..Options::default() };
    let first_line_is_a_value = text.lines().find(|l| !l.trim().is_empty()).is_some_and(|l| {
        let read = parse(l, &options);
        read.root.is_some() && read.diagnostics.iter().all(|d| d.severity() != Severity::Error)
    });
    let multiple = text.lines().filter(|l| !l.trim().is_empty()).count() > 1 && first_line_is_a_value;
    let roots: Vec<Value> = if multiple {
        let mut roots = Vec::new();
        let mut errors = Vec::new();
        for (_, read) in parse_lines(text, &options) {
            errors.extend(read.diagnostics.iter().filter(|d| d.severity() == Severity::Error).map(|d| BsonError {
                offset: d.span.start,
                length: d.span.len(),
                message: d.message(),
            }));
            roots.extend(read.root);
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        roots
    } else if text.trim().is_empty() {
        Vec::new()
    } else {
        let read = parse(text, &options);
        let errors: Vec<BsonError> = read
            .diagnostics
            .iter()
            .filter(|d| d.severity() == Severity::Error)
            .map(|d| BsonError { offset: d.span.start, length: d.span.len(), message: d.message() })
            .collect();
        if !errors.is_empty() {
            return Err(errors);
        }
        read.root.into_iter().collect()
    };
    let mut documents = Vec::new();
    let mut errors = Vec::new();
    for root in &roots {
        match document_of(root) {
            Ok(d) => documents.push(d),
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() { Ok(documents) } else { Err(errors) }
}

fn document_of(value: &Value) -> Result<Document, BsonError> {
    match &value.data {
        Data::Object(_) => match convert(value)? {
            Bson::Document(d) if depth(&d) > crate::MAX_DEPTH => {
                Err(problem(value, format!("documents are nested deeper than {} levels", crate::MAX_DEPTH)))
            }
            Bson::Document(d) => Ok(d),
            _ => Err(problem(value, "a document is an object, not a value of another type")),
        },
        _ => Err(problem(value, "a document is an object")),
    }
}

fn text(value: &Value) -> Option<&str> {
    if let Data::String(s) = &value.data { Some(s) } else { None }
}

fn as_u32(value: &Value) -> Option<u32> {
    if let Data::Number(n) = &value.data { n.parse().ok() } else { None }
}

/// The value of an object that has exactly the keys `keys` (in any order), in the order of `keys`.
fn exact<'v>(members: &'v [xmlcraft_json_syntax::Member], keys: &[&str]) -> Option<Vec<&'v Value>> {
    if members.len() != keys.len() {
        return None;
    }
    keys.iter().map(|k| members.iter().find(|m| m.key == *k).map(|m| &m.value)).collect()
}

fn hex_id(s: &str) -> Option<[u8; 12]> {
    if s.len() != 24 || !s.is_ascii() {
        return None;
    }
    let mut id = [0u8; 12];
    for (i, slot) in id.iter_mut().enumerate() {
        *slot = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(id)
}

fn convert(value: &Value) -> Result<Bson, BsonError> {
    match &value.data {
        Data::Null => Ok(Bson::Null),
        Data::Bool(b) => Ok(Bson::Bool(*b)),
        Data::String(s) => Ok(Bson::String(s.clone())),
        Data::Number(n) => {
            // (An integer in the text; "1.0" and "1e3" are not parsed as one.)
            if let Ok(i) = n.parse::<i32>() {
                return Ok(Bson::Int32(i));
            }
            if let Ok(i) = n.parse::<i64>() {
                return Ok(Bson::Int64(i));
            }
            n.parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .map(Bson::Double)
                .ok_or_else(|| problem(value, format!("'{n}' is not a number BSON can hold")))
        }
        Data::Array(items) => items.iter().map(convert).collect::<Result<Vec<_>, _>>().map(Bson::Array),
        Data::Object(members) => {
            // (`$code` may come after `$scope`.)
            let wrapper = if members.iter().any(|m| m.key == "$code") {
                Some("$code")
            } else {
                members.first().map(|m| m.key.as_str()).filter(|k| k.starts_with('$'))
            };
            match wrapper {
                Some(key) => wrapped(value, members, key),
                None => plain(members),
            }
        }
    }
}

fn plain(members: &[xmlcraft_json_syntax::Member]) -> Result<Bson, BsonError> {
    let mut fields = Vec::with_capacity(members.len());
    for m in members {
        if m.key.contains('\0') {
            return Err(BsonError {
                offset: m.key_span.start,
                length: m.key_span.len(),
                message: "a field name cannot hold a zero character".to_owned(),
            });
        }
        fields.push((m.key.clone(), convert(&m.value)?));
    }
    Ok(Bson::Document(Document { fields }))
}

fn wrapped(value: &Value, members: &[xmlcraft_json_syntax::Member], key: &str) -> Result<Bson, BsonError> {
    let one = |k: &str| exact(members, &[k]).and_then(|v| v.first().copied());
    let bad = |what: &str| problem(value, what.to_owned());
    match key {
        "$oid" => {
            let v = one("$oid").ok_or_else(|| bad("$oid stands alone"))?;
            text(v).and_then(hex_id).map(Bson::ObjectId).ok_or_else(|| problem(v, "an $oid is 24 hexadecimal digits"))
        }
        "$numberInt" => {
            let v = one(key).ok_or_else(|| bad("$numberInt stands alone"))?;
            text(v).and_then(|s| s.parse().ok()).map(Bson::Int32).ok_or_else(|| problem(v, "a $numberInt is a 32-bit integer in a string"))
        }
        "$numberLong" => {
            let v = one(key).ok_or_else(|| bad("$numberLong stands alone"))?;
            text(v).and_then(|s| s.parse().ok()).map(Bson::Int64).ok_or_else(|| problem(v, "a $numberLong is a 64-bit integer in a string"))
        }
        "$numberDouble" => {
            let v = one(key).ok_or_else(|| bad("$numberDouble stands alone"))?;
            let parsed = text(v).and_then(|s| match s {
                "NaN" => Some(f64::NAN),
                "Infinity" => Some(f64::INFINITY),
                "-Infinity" => Some(f64::NEG_INFINITY),
                other => other.parse().ok(),
            });
            parsed.map(Bson::Double).ok_or_else(|| problem(v, "a $numberDouble is a number, NaN, Infinity or -Infinity in a string"))
        }
        "$numberDecimal" => {
            let v = one(key).ok_or_else(|| bad("$numberDecimal stands alone"))?;
            let s = text(v).ok_or_else(|| problem(v, "a $numberDecimal is a string"))?;
            decimal::from_string(s).map(Bson::Decimal128).map_err(|m| problem(v, m))
        }
        "$date" => {
            let v = one(key).ok_or_else(|| bad("$date stands alone"))?;
            match &v.data {
                Data::String(s) => chrono::DateTime::parse_from_rfc3339(s)
                    .map(|d| Bson::DateTime(d.timestamp_millis()))
                    .map_err(|_| problem(v, "a $date is an ISO 8601 date and time, or a $numberLong")),
                Data::Object(inner) => {
                    match exact(inner, &["$numberLong"]).and_then(|v| v.first().copied()).and_then(text).and_then(|s| s.parse().ok()) {
                        Some(ms) => Ok(Bson::DateTime(ms)),
                        None => Err(problem(v, "a $date is an ISO 8601 date and time, or a $numberLong")),
                    }
                }
                _ => Err(problem(v, "a $date is an ISO 8601 date and time, or a $numberLong")),
            }
        }
        "$binary" => {
            let v = one(key).ok_or_else(|| bad("$binary stands alone"))?;
            let Data::Object(inner) = &v.data else { return Err(problem(v, "a $binary has base64 and subType")) };
            let parts = exact(inner, &["base64", "subType"]).ok_or_else(|| problem(v, "a $binary has base64 and subType"))?;
            let bytes = parts
                .first()
                .and_then(|p| text(p))
                .and_then(base64::decode)
                .ok_or_else(|| problem(parts.first().copied().unwrap_or(v), "base64 is not valid base64"))?;
            let subtype = parts.get(1).and_then(|p| text(p)).filter(|s| s.len() <= 2).and_then(|s| u8::from_str_radix(s, 16).ok());
            let subtype = subtype.ok_or_else(|| problem(parts.get(1).copied().unwrap_or(v), "a subType is one or two hexadecimal digits"))?;
            if let Some(m) = binary_problem(subtype, &bytes) {
                return Err(problem(v, m));
            }
            Ok(Bson::Binary { subtype, bytes })
        }
        "$uuid" => {
            let v = one(key).ok_or_else(|| bad("$uuid stands alone"))?;
            let digits: String = text(v).unwrap_or_default().chars().filter(|c| *c != '-').collect();
            let bytes = (digits.len() == 32 && digits.is_ascii())
                .then(|| (0..16).map(|i| digits.get(i * 2..i * 2 + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect::<Option<Vec<u8>>>())
                .flatten();
            bytes
                .map(|bytes| Bson::Binary { subtype: 4, bytes })
                .ok_or_else(|| problem(v, "a $uuid is 32 hexadecimal digits, with or without dashes"))
        }
        "$regularExpression" => {
            let v = one(key).ok_or_else(|| bad("$regularExpression stands alone"))?;
            let Data::Object(inner) = &v.data else { return Err(problem(v, "a $regularExpression has pattern and options")) };
            let parts = exact(inner, &["pattern", "options"]).ok_or_else(|| problem(v, "a $regularExpression has pattern and options"))?;
            match (parts.first().and_then(|p| text(p)), parts.get(1).and_then(|p| text(p))) {
                (Some(pattern), Some(options)) if !pattern.contains('\0') && !options.contains('\0') => {
                    Ok(Bson::Regex { pattern: pattern.to_owned(), options: options.to_owned() })
                }
                _ => Err(problem(v, "pattern and options are strings without a zero character")),
            }
        }
        "$timestamp" => {
            let v = one(key).ok_or_else(|| bad("$timestamp stands alone"))?;
            let Data::Object(inner) = &v.data else { return Err(problem(v, "a $timestamp has t and i")) };
            let parts = exact(inner, &["t", "i"]).ok_or_else(|| problem(v, "a $timestamp has t and i"))?;
            match (parts.first().and_then(|p| as_u32(p)), parts.get(1).and_then(|p| as_u32(p))) {
                (Some(time), Some(increment)) => Ok(Bson::Timestamp { time, increment }),
                _ => Err(problem(v, "t and i are unsigned 32-bit integers")),
            }
        }
        "$undefined" => match one(key).map(|v| &v.data) {
            Some(Data::Bool(true)) => Ok(Bson::Undefined),
            _ => Err(bad("$undefined stands alone, with the value true")),
        },
        "$minKey" | "$maxKey" => match one(key).map(|v| &v.data) {
            Some(Data::Number(n)) if n == "1" => Ok(if key == "$minKey" { Bson::MinKey } else { Bson::MaxKey }),
            _ => Err(bad("$minKey and $maxKey stand alone, with the value 1")),
        },
        "$symbol" => {
            let v = one(key).ok_or_else(|| bad("$symbol stands alone"))?;
            text(v).map(|s| Bson::Symbol(s.to_owned())).ok_or_else(|| problem(v, "a $symbol is a string"))
        }
        "$code" => {
            if members.len() == 1 {
                let v = one(key).ok_or_else(|| bad("$code"))?;
                return text(v).map(|s| Bson::JavaScript(s.to_owned())).ok_or_else(|| problem(v, "$code is a string"));
            }
            let parts = exact(members, &["$code", "$scope"]).ok_or_else(|| bad("$code stands alone or with $scope"))?;
            let code = parts.first().and_then(|p| text(p)).ok_or_else(|| problem(parts.first().copied().unwrap_or(value), "$code is a string"))?;
            let scope = parts.get(1).copied().ok_or_else(|| bad("$scope"))?;
            Ok(Bson::JavaScriptWithScope { code: code.to_owned(), scope: document_of(scope)? })
        }
        "$dbPointer" => {
            let v = one(key).ok_or_else(|| bad("$dbPointer stands alone"))?;
            let Data::Object(inner) = &v.data else { return Err(problem(v, "a $dbPointer has $ref and $id")) };
            let parts = exact(inner, &["$ref", "$id"]).ok_or_else(|| problem(v, "a $dbPointer has $ref and $id"))?;
            let namespace = parts.first().and_then(|p| text(p));
            let id = parts
                .get(1)
                .and_then(|p| if let Data::Object(m) = &p.data { exact(m, &["$oid"]) } else { None })
                .and_then(|v| v.first().copied())
                .and_then(text)
                .and_then(hex_id);
            match (namespace, id) {
                (Some(ns), Some(id)) => Ok(Bson::DbPointer { namespace: ns.to_owned(), id }),
                _ => Err(problem(v, "a $dbPointer has a $ref string and an $id that is an $oid")),
            }
        }
        // A field whose name starts with `$` and means nothing special is an ordinary document.
        _ => plain(members),
    }
}
