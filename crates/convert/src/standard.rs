//! The standard mapping between JSON and XML of XPath 3.1 (`fn:json-to-xml` and `fn:xml-to-json`): lossless for JSON.
//!
//! `{"a": [1, "x", null, true]}` is
//! `<map xmlns="http://www.w3.org/2005/xpath-functions"><array key="a"><number>1</number><string>x</string><null/><boolean>true</boolean></array></map>`.

use xmlcraft_json_syntax::{Data, Member, Value};
use xmlcraft_syntax::{SyntaxKind, SyntaxNode, decode_references, parse};

use crate::{ConvertError, error};

const NS: &str = "http://www.w3.org/2005/xpath-functions";
const MAX_DEPTH: usize = 256;

fn val(data: Data) -> Value {
    Value { span: 0..0, data }
}

fn escape(s: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            c if !xmlcraft_syntax::is_xml_char(c) => {}
            c => out.push(c),
        }
    }
    out
}

fn write(value: &Value, key: Option<&str>, depth: usize, indent: Option<usize>, out: &mut String, root: bool) -> Result<(), ConvertError> {
    if depth > MAX_DEPTH {
        return Err(error(None, "the data is nested too deeply"));
    }
    if let Some(n) = indent {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&" ".repeat(n.saturating_mul(depth)));
    }
    let attrs = |key: Option<&str>| -> String {
        let mut s = String::new();
        if root {
            s.push_str(&format!(" xmlns=\"{NS}\""));
        }
        if let Some(k) = key {
            s.push_str(&format!(" key=\"{}\"", escape(k, true)));
        }
        s
    };
    let a = attrs(key);
    match &value.data {
        Data::Null => out.push_str(&format!("<null{a}/>")),
        Data::Bool(b) => out.push_str(&format!("<boolean{a}>{b}</boolean>")),
        Data::Number(n) => {
            if !n.parse::<f64>().is_ok_and(f64::is_finite) {
                return Err(error(None, format!("{n} is a number JSON cannot hold")));
            }
            out.push_str(&format!("<number{a}>{}</number>", n.strip_prefix('+').unwrap_or(n)));
        }
        Data::String(s) => out.push_str(&format!("<string{a}>{}</string>", escape(s, false))),
        Data::Array(items) => {
            if items.is_empty() {
                out.push_str(&format!("<array{a}/>"));
                return Ok(());
            }
            out.push_str(&format!("<array{a}>"));
            for item in items {
                write(item, None, depth + 1, indent, out, false)?;
            }
            if let Some(n) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(n.saturating_mul(depth)));
            }
            out.push_str("</array>");
        }
        Data::Object(members) => {
            if members.is_empty() {
                out.push_str(&format!("<map{a}/>"));
                return Ok(());
            }
            out.push_str(&format!("<map{a}>"));
            let mut seen = std::collections::HashSet::new();
            for m in members {
                if seen.insert(m.key.as_str()) {
                    write(&m.value, Some(&m.key), depth + 1, indent, out, false)?;
                }
            }
            if let Some(n) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(n.saturating_mul(depth)));
            }
            out.push_str("</map>");
        }
    }
    Ok(())
}

/// The standard XML for a JSON value: the element in the namespace of the XPath functions, with `key` attributes. Duplicate names in an
/// object are not allowed by the mapping: the first stays.
pub fn standard_to_xml(value: &Value, indent: Option<usize>, _warnings: &mut Vec<String>) -> Result<String, ConvertError> {
    let mut out = String::new();
    write(value, None, 0, indent, &mut out, true)?;
    if indent.is_some() {
        out.push('\n');
    }
    Ok(out)
}

fn attribute(tag: &SyntaxNode, name: &str) -> Option<String> {
    tag.children().filter(|n| n.kind() == SyntaxKind::Attribute).find_map(|a| {
        let attr_name = a.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name)?;
        (attr_name.text() == name).then(|| {
            let raw = a.children().find(|n| n.kind() == SyntaxKind::AttrValue).map(|v| v.text().to_string()).unwrap_or_default();
            decode_references(raw.trim_matches(|c| c == '"' || c == '\''))
        })
    })
}

fn local(element: &SyntaxNode) -> Option<(String, SyntaxNode)> {
    let tag = element.children().find(|n| n.kind() == SyntaxKind::StartTag)?;
    let name = tag.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name)?;
    let text = name.text();
    Some((text.rsplit_once(':').map_or(text, |(_, l)| l).to_owned(), tag))
}

fn read(element: &SyntaxNode, depth: usize) -> Result<Value, ConvertError> {
    let at = Some(usize::from(element.text_range().start()));
    if depth > MAX_DEPTH {
        return Err(error(at, "elements are nested too deeply"));
    }
    let (name, _) = local(element).ok_or_else(|| error(at, "an element has no name"))?;
    let text: String = element
        .children_with_tokens()
        .filter_map(|c| match c {
            rowan::NodeOrToken::Token(t) if matches!(t.kind(), SyntaxKind::Text | SyntaxKind::CharRef | SyntaxKind::EntityRef) => {
                Some(decode_references(t.text()))
            }
            rowan::NodeOrToken::Node(n) if n.kind() == SyntaxKind::CData => Some(
                n.children_with_tokens()
                    .filter_map(|c| c.into_token())
                    .filter(|t| t.kind() == SyntaxKind::CdataText)
                    .map(|t| t.text().to_owned())
                    .collect(),
            ),
            _ => None,
        })
        .collect();
    let children: Vec<SyntaxNode> = element.children().filter(|n| n.kind() == SyntaxKind::Element).collect();
    match name.as_str() {
        "null" => Ok(val(Data::Null)),
        "boolean" => match text.trim() {
            "true" | "1" => Ok(val(Data::Bool(true))),
            "false" | "0" => Ok(val(Data::Bool(false))),
            other => Err(error(at, format!("'{other}' is not a boolean"))),
        },
        "number" => {
            let t = text.trim();
            if t.parse::<f64>().is_ok_and(f64::is_finite) {
                Ok(val(Data::Number(t.to_owned())))
            } else {
                Err(error(at, format!("'{t}' is not a number")))
            }
        }
        "string" => Ok(val(Data::String(text))),
        "array" => children.iter().map(|c| read(c, depth + 1)).collect::<Result<Vec<_>, _>>().map(|items| val(Data::Array(items))),
        "map" => {
            let mut members = Vec::new();
            for child in &children {
                let (_, tag) = local(child).ok_or_else(|| error(at, "an element has no name"))?;
                let key = attribute(&tag, "key")
                    .ok_or_else(|| error(Some(usize::from(child.text_range().start())), "an item of a map needs a key attribute"))?;
                members.push(Member { key, key_span: 0..0, value: read(child, depth + 1)? });
            }
            Ok(val(Data::Object(members)))
        }
        other => Err(error(at, format!("<{other}> is not part of the XPath 3.1 JSON mapping (map, array, string, number, boolean, null)"))),
    }
}

/// The JSON value of XML in the standard mapping (`fn:xml-to-json`).
pub fn standard_to_value(text: &str) -> Result<Value, ConvertError> {
    let parsed = parse(text);
    if let Some(d) = parsed.diagnostics().iter().find(|d| d.severity() == xmlcraft_syntax::Severity::Error) {
        return Err(error(Some(d.span.start), format!("the XML is not well-formed: {}", d.message())));
    }
    let root = parsed.syntax();
    let element = root.children().find(|n| n.kind() == SyntaxKind::Element).ok_or_else(|| error(None, "the XML has no element"))?;
    read(&element, 0)
}
