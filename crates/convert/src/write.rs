//! Writing a value tree as JSON or YAML text.

use xmlcraft_json_syntax::{Data, Value};

use crate::{ConvertError, error};

fn quote(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// JSON text. `indent` is the spaces for each level, or none for one compact line. A number JSON cannot hold (`NaN`, `Infinity`) is an
/// error; a JSON5 number such as `+1` or `0x10` is written the way JSON writes it.
pub fn json_text(value: &Value, indent: Option<usize>, warnings: &mut Vec<String>) -> Result<String, ConvertError> {
    let mut out = String::new();
    json(value, indent, 0, &mut out, warnings)?;
    if indent.is_some() {
        out.push('\n');
    }
    Ok(out)
}

/// A number as JSON writes it, or none for `NaN` and the infinities.
fn json_number(text: &str) -> Option<String> {
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u128::from_str_radix(hex, 16).ok().map(|v| v.to_string());
    }
    let body = text.strip_prefix('+').unwrap_or(text);
    let valid = body.parse::<f64>().is_ok_and(f64::is_finite) && !body.starts_with('.') && !body.ends_with('.') && !body.starts_with("-.");
    valid.then(|| body.to_owned())
}

fn json(value: &Value, indent: Option<usize>, level: usize, out: &mut String, warnings: &mut Vec<String>) -> Result<(), ConvertError> {
    let pad = |n: usize| " ".repeat(indent.unwrap_or(0).saturating_mul(n));
    match &value.data {
        Data::Null => out.push_str("null"),
        Data::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Data::Number(n) => match json_number(n) {
            Some(text) => out.push_str(&text),
            None => return Err(error(None, format!("{n} is a number JSON cannot hold"))),
        },
        Data::String(s) => quote(s, out),
        Data::Array(items) if items.is_empty() => out.push_str("[]"),
        Data::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if indent.is_some() {
                    out.push('\n');
                    out.push_str(&pad(level + 1));
                }
                json(item, indent, level + 1, out, warnings)?;
            }
            if indent.is_some() {
                out.push('\n');
                out.push_str(&pad(level));
            }
            out.push(']');
        }
        Data::Object(members) if members.is_empty() => out.push_str("{}"),
        Data::Object(members) => {
            out.push('{');
            let mut seen = std::collections::HashSet::new();
            let mut first = true;
            for m in members {
                if !seen.insert(m.key.as_str()) {
                    let note = format!("the name '{}' was used more than once in an object; only the first is written", m.key);
                    if !warnings.contains(&note) {
                        warnings.push(note);
                    }
                    continue;
                }
                if !first {
                    out.push(',');
                }
                first = false;
                if indent.is_some() {
                    out.push('\n');
                    out.push_str(&pad(level + 1));
                }
                quote(&m.key, out);
                out.push_str(if indent.is_some() { ": " } else { ":" });
                json(&m.value, indent, level + 1, out, warnings)?;
            }
            if indent.is_some() {
                out.push('\n');
                out.push_str(&pad(level));
            }
            out.push('}');
        }
    }
    Ok(())
}

/// YAML text (block style, two spaces). Strings are written plain when they read back as the same string, else in double quotes.
pub fn yaml_text(value: &Value, warnings: &mut Vec<String>) -> String {
    let mut out = String::new();
    match &value.data {
        Data::Array(items) if !items.is_empty() => seq(items, 0, &mut out, warnings),
        Data::Object(members) if !members.is_empty() => map(members, 0, &mut out, warnings),
        _ => {
            out.push_str(&scalar(value));
            out.push('\n');
        }
    }
    out
}

/// Whether a string is written plain in YAML and reads back as the same string.
fn plain_ok(s: &str) -> bool {
    if s.is_empty() || s != s.trim() || s.contains(['\n', '\r', '\t']) || s.chars().any(char::is_control) {
        return false;
    }
    let first = s.chars().next().unwrap_or(' ');
    if "-?:,[]{}#&*!|>'\"%@`".contains(first) && !(matches!(first, '-' | '?' | ':') && s.chars().nth(1).is_some_and(|c| !c.is_whitespace())) {
        return false;
    }
    // A document marker (`--- x`, `...`) at the start of a line is not text.
    let marker = ["---", "..."].iter().any(|m| s.strip_prefix(m).is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t'])));
    if marker || s.contains(": ") || s.contains(" #") || s.ends_with(':') || s == "-" {
        return false;
    }
    // Anything the reader would make something else of (null, booleans, numbers) or the older spellings others read.
    let lower = s.to_ascii_lowercase();
    if matches!(lower.as_str(), "yes" | "no" | "on" | "off" | "y" | "n" | "~" | "null" | "true" | "false" | ".nan" | ".inf" | "-.inf" | "+.inf") {
        return false;
    }
    xmlcraft_yaml_syntax::plain_data(s).is_none_or(|d| matches!(d, Data::String(_)))
        && !s.starts_with("0x")
        && !s.starts_with("0o")
        && !looks_numeric(s)
}

fn looks_numeric(s: &str) -> bool {
    s.parse::<f64>().is_ok()
        || s.trim_start_matches(['+', '-']).chars().all(|c| c.is_ascii_digit() || c == '_' || c == '.') && s.chars().any(|c| c.is_ascii_digit())
}

fn key(k: &str) -> String {
    if plain_ok(k) {
        k.to_owned()
    } else {
        let mut out = String::new();
        quote(k, &mut out);
        out
    }
}

fn scalar(value: &Value) -> String {
    match &value.data {
        Data::Null => "null".to_owned(),
        Data::Bool(b) => b.to_string(),
        Data::Number(n) => match n.as_str() {
            "Infinity" | "+Infinity" => ".inf".to_owned(),
            "-Infinity" => "-.inf".to_owned(),
            "NaN" => ".nan".to_owned(),
            other => json_number(other).unwrap_or_else(|| other.to_owned()),
        },
        Data::String(s) if plain_ok(s) => s.clone(),
        Data::String(s) => {
            let mut out = String::new();
            quote(s, &mut out);
            out
        }
        Data::Array(_) => "[]".to_owned(),
        Data::Object(_) => "{}".to_owned(),
    }
}

fn is_container(v: &Value) -> bool {
    matches!(&v.data, Data::Array(a) if !a.is_empty()) || matches!(&v.data, Data::Object(o) if !o.is_empty())
}

fn map(members: &[xmlcraft_json_syntax::Member], level: usize, out: &mut String, warnings: &mut Vec<String>) {
    let pad = "  ".repeat(level);
    let mut seen = std::collections::HashSet::new();
    for m in members {
        if !seen.insert(m.key.as_str()) {
            let note = format!("the name '{}' was used more than once in an object; only the first is written", m.key);
            if !warnings.contains(&note) {
                warnings.push(note);
            }
            continue;
        }
        out.push_str(&pad);
        out.push_str(&key(&m.key));
        out.push(':');
        entry(&m.value, level, out, warnings, false);
    }
}

fn seq(items: &[Value], level: usize, out: &mut String, warnings: &mut Vec<String>) {
    let pad = "  ".repeat(level);
    for item in items {
        out.push_str(&pad);
        out.push('-');
        entry(item, level, out, warnings, true);
    }
}

/// What follows `key:` or `-`: a scalar on the same line, or a block on the lines below (for a list entry that is a mapping, its first
/// member on the line of the dash).
fn entry(value: &Value, level: usize, out: &mut String, warnings: &mut Vec<String>, in_list: bool) {
    if !is_container(value) {
        out.push(' ');
        out.push_str(&scalar(value));
        out.push('\n');
        return;
    }
    match &value.data {
        Data::Object(members) if in_list => {
            // `- a: 1` and the rest of the members aligned under `a`.
            let mut inner = String::new();
            map(members, level + 1, &mut inner, warnings);
            let trimmed = inner.get("  ".len() * (level + 1)..).unwrap_or(&inner);
            out.push(' ');
            out.push_str(trimmed);
        }
        Data::Object(members) => {
            out.push('\n');
            map(members, level + 1, out, warnings);
        }
        Data::Array(items) if in_list => {
            let mut inner = String::new();
            seq(items, level + 1, &mut inner, warnings);
            let trimmed = inner.get("  ".len() * (level + 1)..).unwrap_or(&inner);
            out.push(' ');
            out.push_str(trimmed);
        }
        Data::Array(items) => {
            out.push('\n');
            seq(items, level + 1, out, warnings);
        }
        _ => {}
    }
}
