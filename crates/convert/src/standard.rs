//! The standard mapping between JSON and XML of XPath 3.1 (`fn:json-to-xml` and `fn:xml-to-json`): lossless for JSON.
//!
//! `{"a": [1, "x", null, true]}` is
//! `<map xmlns="http://www.w3.org/2005/xpath-functions"><array key="a"><number>1</number><string>x</string><null/><boolean>true</boolean></array></map>`.

use xmlcraft_json_syntax::{Data, Member, Value};
use xmlcraft_syntax::{SyntaxKind, SyntaxNode, decode_references, parse};

use crate::xml::is_json_number;
use crate::{ConvertError, MAX_INDENT, error, unquote};

const NS: &str = "http://www.w3.org/2005/xpath-functions";
const MAX_DEPTH: usize = 256;

fn val(data: Data) -> Value {
    Value { span: 0..0, data }
}

/// `s` as XML text or attribute value. The mapping is lossless, so a character XML cannot hold is refused, and the line breaks and tabs
/// of an attribute value are written as references (a parser would make spaces of them).
fn escape(s: &str, attribute: bool) -> Result<String, ConvertError> {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\t' if attribute => out.push_str("&#9;"),
            '\r' => out.push_str("&#13;"),
            c if !xmlcraft_syntax::is_xml_char(c) => {
                return Err(error(
                    None,
                    format!("U+{:04X} cannot be written in XML, so the text cannot be converted without losing it", u32::from(c)),
                ));
            }
            c => out.push(c),
        }
    }
    Ok(out)
}

fn write(
    value: &Value,
    key: Option<&str>,
    depth: usize,
    indent: Option<usize>,
    out: &mut String,
    root: bool,
    warnings: &mut Vec<String>,
) -> Result<(), ConvertError> {
    if depth > MAX_DEPTH {
        return Err(error(None, "the data is nested too deeply"));
    }
    if let Some(n) = indent {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&" ".repeat(n.saturating_mul(depth)));
    }
    let mut a = String::new();
    if root {
        a.push_str(&format!(" xmlns=\"{NS}\""));
    }
    if let Some(k) = key {
        a.push_str(&format!(" key=\"{}\"", escape(k, true)?));
    }
    match &value.data {
        Data::Null => out.push_str(&format!("<null{a}/>")),
        Data::Bool(b) => out.push_str(&format!("<boolean{a}>{b}</boolean>")),
        Data::Number(n) => {
            // The number is written the JSON way (what JSON5 and YAML allow beyond JSON is not XML Schema's either).
            let text = crate::write::json_number(n).ok_or_else(|| error(None, format!("{n} is a number JSON cannot hold")))?;
            out.push_str(&format!("<number{a}>{text}</number>"));
        }
        Data::String(s) => out.push_str(&format!("<string{a}>{}</string>", escape(s, false)?)),
        Data::Array(items) => {
            if items.is_empty() {
                out.push_str(&format!("<array{a}/>"));
                return Ok(());
            }
            out.push_str(&format!("<array{a}>"));
            for item in items {
                write(item, None, depth + 1, indent, out, false, warnings)?;
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
                    write(&m.value, Some(&m.key), depth + 1, indent, out, false, warnings)?;
                } else {
                    let note = format!("the name '{}' was used more than once in an object; only the first is written", m.key);
                    if !warnings.contains(&note) {
                        warnings.push(note);
                    }
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
pub fn standard_to_xml(value: &Value, indent: Option<usize>, warnings: &mut Vec<String>) -> Result<String, ConvertError> {
    let indent = indent.map(|n| n.min(MAX_INDENT));
    let mut out = String::new();
    write(value, None, 0, indent, &mut out, true, warnings)?;
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
            decode_references(unquote(&raw))
        })
    })
}

fn local(element: &SyntaxNode) -> Option<(String, SyntaxNode)> {
    let tag = element.children().find(|n| n.kind() == SyntaxKind::StartTag)?;
    let name = tag.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name)?;
    let text = name.text();
    Some((text.rsplit_once(':').map_or(text, |(_, l)| l).to_owned(), tag))
}

/// The namespace the name of `element` is in, by the declarations of the element and those around it.
fn namespace_of(element: &SyntaxNode) -> Option<String> {
    let tag = element.children().find(|n| n.kind() == SyntaxKind::StartTag)?;
    let name = tag.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name)?;
    let declaration = match name.text().split_once(':') {
        Some((prefix, _)) => format!("xmlns:{prefix}"),
        None => "xmlns".to_owned(),
    };
    element
        .ancestors()
        .filter(|n| n.kind() == SyntaxKind::Element)
        .find_map(|e| attribute(&e.children().find(|n| n.kind() == SyntaxKind::StartTag)?, &declaration))
}

fn read(element: &SyntaxNode, depth: usize) -> Result<Value, ConvertError> {
    let at = Some(usize::from(element.text_range().start()));
    if depth > MAX_DEPTH {
        return Err(error(at, "elements are nested too deeply"));
    }
    let (name, _) = local(element).ok_or_else(|| error(at, "an element has no name"))?;
    if namespace_of(element).as_deref() != Some(NS) {
        return Err(error(at, format!("<{name}> is not in the namespace {NS} of the XPath 3.1 JSON mapping")));
    }
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
    // The leaves have text and no elements (a null has neither); the containers have elements and no text but white space.
    let is_container = matches!(name.as_str(), "array" | "map");
    if !is_container && let Some(child) = children.first() {
        return Err(error(Some(usize::from(child.text_range().start())), format!("<{name}> cannot contain elements")));
    }
    if is_container && !text.trim().is_empty() {
        return Err(error(at, format!("<{name}> cannot contain text")));
    }
    if name == "null" && !text.trim().is_empty() {
        return Err(error(at, "<null> must be empty"));
    }
    match name.as_str() {
        "null" => Ok(val(Data::Null)),
        "boolean" => match text.trim() {
            "true" | "1" => Ok(val(Data::Bool(true))),
            "false" | "0" => Ok(val(Data::Bool(false))),
            other => Err(error(at, format!("'{other}' is not a boolean"))),
        },
        "number" => {
            let t = text.trim();
            if is_json_number(t) { Ok(val(Data::Number(t.to_owned()))) } else { Err(error(at, format!("'{t}' is not a number"))) }
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
