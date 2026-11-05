//! Reading a SOAP message: the envelope of a response (or of a request), its header blocks, its body, and the fault it may carry.

use xmlcraft_syntax::{Severity, parse};
use xmlcraft_xsd::dom::{self, Dom, El};

use crate::{SOAP11_ENVELOPE, SOAP12_ENVELOPE, SoapVersion};

/// A SOAP fault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    /// SOAP 1.1 `faultcode` (`soap:Server`), SOAP 1.2 `Code/Value` (`soap:Receiver`).
    pub code: String,
    /// SOAP 1.2 `Code/Subcode/Value`, when there is one.
    pub subcode: Option<String>,
    /// SOAP 1.1 `faultstring`, SOAP 1.2 `Reason/Text` (the first one).
    pub reason: String,
    /// SOAP 1.1 `faultactor`, SOAP 1.2 `Role`.
    pub actor: Option<String>,
    /// The XML inside `detail` (1.1) or `Detail` (1.2) as written, when there is any.
    pub detail: Option<String>,
}

/// A SOAP message read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub version: SoapVersion,
    /// Each header block as written.
    pub headers: Vec<String>,
    /// What is in the body as written (the XML between `<Body>` and `</Body>`, trimmed).
    pub body: String,
    pub fault: Option<Fault>,
}

fn child<'a>(tree: &'a Dom, el: &El, namespace: &str, local: &str) -> Option<&'a El> {
    el.children.iter().filter_map(|i| tree.elements.get(*i)).find(|c| c.local == local && c.namespace.as_deref() == Some(namespace))
}

/// The text between the end of the start tag and the start of the end tag of `el` (empty for `<a/>`).
fn inner<'a>(text: &'a str, el: &El) -> &'a str {
    let from = el.attrs.iter().map(|a| a.value_range.end).chain(std::iter::once(el.name_range.end)).max().unwrap_or(el.name_range.end);
    let Some(open_end) = text.get(from..el.range.end).and_then(|rest| rest.find('>')).map(|gt| from + gt + 1) else { return "" };
    if text.get(open_end.saturating_sub(2)..open_end) == Some("/>") {
        return "";
    }
    let end = text.get(open_end..el.range.end).and_then(|c| c.rfind("</")).map_or(el.range.end, |lt| open_end + lt);
    text.get(open_end..end).unwrap_or_default()
}

fn element_text(tree: &Dom, el: &El) -> String {
    tree.all_text.get(el.deep.clone()).unwrap_or_default().trim().to_owned()
}

/// Read the SOAP message `text`: its version from the namespace of the envelope.
pub fn read_message(text: &str) -> Result<Message, String> {
    let parsed = parse(text);
    if parsed.diagnostics().iter().any(|d| d.severity() == Severity::Error) {
        return Err("the message is not well-formed XML".to_owned());
    }
    let tree = dom::build(&parsed.syntax());
    let Some(root) = tree.elements.first() else { return Err("the message is empty".to_owned()) };
    let version = match (root.namespace.as_deref(), root.local.as_str()) {
        (Some(SOAP11_ENVELOPE), "Envelope") => SoapVersion::V11,
        (Some(SOAP12_ENVELOPE), "Envelope") => SoapVersion::V12,
        _ => return Err("this is not a SOAP message (the root is not an Envelope of SOAP 1.1 or 1.2)".to_owned()),
    };
    let ns = version.envelope_namespace();
    let headers: Vec<String> = match child(&tree, root, ns, "Header") {
        Some(h) => h.children.iter().filter_map(|i| tree.elements.get(*i)).filter_map(|b| text.get(b.range.clone())).map(str::to_owned).collect(),
        None => Vec::new(),
    };
    let Some(body) = child(&tree, root, ns, "Body") else { return Err("the envelope has no Body".to_owned()) };
    let fault = child(&tree, body, ns, "Fault").map(|f| match version {
        SoapVersion::V11 => {
            // The children of a 1.1 fault are in no namespace.
            let find = |name: &str| f.children.iter().filter_map(|i| tree.elements.get(*i)).find(|c| c.local == name);
            Fault {
                code: find("faultcode").map(|e| element_text(&tree, e)).unwrap_or_default(),
                subcode: None,
                reason: find("faultstring").map(|e| element_text(&tree, e)).unwrap_or_default(),
                actor: find("faultactor").map(|e| element_text(&tree, e)),
                detail: find("detail").map(|e| inner(text, e).trim().to_owned()).filter(|d| !d.is_empty()),
            }
        }
        SoapVersion::V12 => {
            let code = child(&tree, f, ns, "Code");
            let value = code.and_then(|c| child(&tree, c, ns, "Value")).map(|e| element_text(&tree, e)).unwrap_or_default();
            let subcode =
                code.and_then(|c| child(&tree, c, ns, "Subcode")).and_then(|s| child(&tree, s, ns, "Value")).map(|e| element_text(&tree, e));
            let reason = child(&tree, f, ns, "Reason").and_then(|r| child(&tree, r, ns, "Text")).map(|e| element_text(&tree, e)).unwrap_or_default();
            Fault {
                code: value,
                subcode,
                reason,
                actor: child(&tree, f, ns, "Role").map(|e| element_text(&tree, e)),
                detail: child(&tree, f, ns, "Detail").map(|e| inner(text, e).trim().to_owned()).filter(|d| !d.is_empty()),
            }
        }
    });
    Ok(Message { version, headers, body: inner(text, body).trim().to_owned(), fault })
}
