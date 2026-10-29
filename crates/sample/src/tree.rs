//! The document that is built, and its text.

use std::collections::BTreeMap;

use xmlcraft_xsd::model::QName;

pub const XSI_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema-instance";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// The value of an attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttrValue {
    Text(String),
    /// A qualified name, written with whatever prefix its namespace gets (`xsi:type`).
    Name(QName),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub name: QName,
    pub attributes: Vec<(QName, AttrValue)>,
    pub text: Option<String>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(name: QName) -> Self {
        Self { name, attributes: Vec::new(), text: None, children: Vec::new() }
    }
}

fn escape(text: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\t' if attribute => out.push_str("&#9;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    out
}

/// Prefixes for the namespaces a document uses: `xsi` and `xml` keep theirs, the others are `ns1`, `ns2`, … in the order met.
struct Prefixes(BTreeMap<String, String>);

impl Prefixes {
    fn collect(root: &Node, extra: &[&str]) -> Self {
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        let mut next = 0usize;
        let mut note = |ns: &str, map: &mut BTreeMap<String, String>| {
            if map.contains_key(ns) {
                return;
            }
            let prefix = match ns {
                XSI_NAMESPACE => "xsi".to_owned(),
                XML_NAMESPACE => "xml".to_owned(),
                _ => {
                    next += 1;
                    format!("ns{next}")
                }
            };
            map.insert(ns.to_owned(), prefix);
        };
        for ns in extra {
            note(ns, &mut map);
        }
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if let Some(ns) = &node.name.namespace {
                note(ns, &mut map);
            }
            for (name, value) in &node.attributes {
                if let Some(ns) = &name.namespace {
                    note(ns, &mut map);
                }
                if let AttrValue::Name(q) = value
                    && let Some(ns) = &q.namespace
                {
                    note(ns, &mut map);
                }
            }
            // Children in reverse, so that the order met is the order of the document.
            stack.extend(node.children.iter().rev());
        }
        Prefixes(map)
    }

    fn qualified(&self, name: &QName) -> String {
        match name.namespace.as_ref().and_then(|ns| self.0.get(ns)) {
            Some(p) => format!("{p}:{}", name.local),
            None => name.local.clone(),
        }
    }
}

/// The text of a document: the root declares every namespace, elements are indented, text stays on the line of its element.
/// `schema_location` adds the hint that names the schema (`xsi:schemaLocation` or `xsi:noNamespaceSchemaLocation`).
pub fn serialize(root: &Node, schema_location: Option<&str>) -> String {
    let wants_xsi = schema_location.is_some();
    let prefixes = Prefixes::collect(root, if wants_xsi { &[XSI_NAMESPACE] } else { &[] });
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    write_node(root, &prefixes, 0, true, schema_location, &mut out);
    out
}

fn write_node(node: &Node, prefixes: &Prefixes, depth: usize, is_root: bool, schema_location: Option<&str>, out: &mut String) {
    let indent = "  ".repeat(depth);
    let name = prefixes.qualified(&node.name);
    out.push_str(&format!("{indent}<{name}"));
    if is_root {
        // The prefixes in a stable order.
        let mut declared: Vec<(&String, &String)> = prefixes.0.iter().filter(|(ns, _)| ns.as_str() != XML_NAMESPACE).map(|(ns, p)| (p, ns)).collect();
        declared.sort();
        for (prefix, ns) in declared {
            out.push_str(&format!(" xmlns:{prefix}=\"{}\"", escape(ns, true)));
        }
        if let Some(location) = schema_location {
            match &node.name.namespace {
                Some(ns) => out.push_str(&format!(" xsi:schemaLocation=\"{} {}\"", escape(ns, true), escape(location, true))),
                None => out.push_str(&format!(" xsi:noNamespaceSchemaLocation=\"{}\"", escape(location, true))),
            }
        }
    }
    for (attribute, value) in &node.attributes {
        let text = match value {
            AttrValue::Text(t) => escape(t, true),
            AttrValue::Name(q) => escape(&prefixes.qualified(q), true),
        };
        out.push_str(&format!(" {}=\"{text}\"", prefixes.qualified(attribute)));
    }
    if node.children.is_empty() {
        match &node.text {
            Some(text) if !text.is_empty() => out.push_str(&format!(">{}</{name}>\n", escape(text, false))),
            _ => out.push_str("/>\n"),
        }
        return;
    }
    out.push_str(">\n");
    if let Some(text) = &node.text
        && !text.is_empty()
    {
        out.push_str(&format!("{indent}  {}\n", escape(text, false)));
    }
    for child in &node.children {
        write_node(child, prefixes, depth + 1, false, schema_location, out);
    }
    out.push_str(&format!("{indent}</{name}>\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(ns: Option<&str>, local: &str) -> QName {
        QName::new(ns, local)
    }

    #[test]
    fn namespaces_are_declared_on_the_root_with_prefixes_and_no_namespace_stays_bare() {
        let mut root = Node::new(q(Some("urn:a"), "r"));
        let mut child = Node::new(q(None, "plain"));
        child.text = Some("a < b & c".to_owned());
        root.children.push(child);
        let mut other = Node::new(q(Some("urn:b"), "o"));
        other.attributes.push((q(Some("urn:a"), "x"), AttrValue::Text("say \"hi\"".to_owned())));
        other.attributes.push((q(Some(XSI_NAMESPACE), "type"), AttrValue::Name(q(Some("urn:b"), "T"))));
        root.children.push(other);
        let text = serialize(&root, None);
        let expected = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ns1:r xmlns:ns1=\"urn:a\" xmlns:ns2=\"urn:b\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n  <plain>a &lt; b &amp; c</plain>\n  <ns2:o ns1:x=\"say &quot;hi&quot;\" xsi:type=\"ns2:T\"/>\n</ns1:r>\n";
        assert_eq!(text, expected);
    }

    #[test]
    fn the_schema_location_hint_follows_the_namespace_of_the_root() {
        let none = serialize(&Node::new(q(None, "r")), Some("r.xsd"));
        assert!(none.contains("xsi:noNamespaceSchemaLocation=\"r.xsd\"") && none.contains("xmlns:xsi="), "{none}");
        let some = serialize(&Node::new(q(Some("urn:a"), "r")), Some("a b.xsd"));
        assert!(some.contains("xsi:schemaLocation=\"urn:a a b.xsd\""), "{some}");
        assert!(!serialize(&Node::new(q(None, "r")), None).contains("xsi"));
    }

    #[test]
    fn text_next_to_children_goes_on_its_own_line() {
        let mut root = Node::new(q(None, "r"));
        root.text = Some("hello".to_owned());
        root.children.push(Node::new(q(None, "c")));
        assert_eq!(serialize(&root, None), "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<r>\n  hello\n  <c/>\n</r>\n");
    }
}
