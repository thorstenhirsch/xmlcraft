//! Assigning a DTD or an XML Schema to a document, as minimal edits: a DOCTYPE before the root element, or the attributes
//! `xsi:noNamespaceSchemaLocation` / `xsi:schemaLocation` on it. Nothing else of the text is touched: the other lines, the line endings
//! and the white space stay as they are.

use std::fmt;
use std::ops::Range;
use std::path::{Component, Path};

use xmlcraft_dtd::doctype_parts;
use xmlcraft_syntax::{Severity, SyntaxKind, SyntaxNode, decode_references, parse};
use xmlcraft_text::Edit;
use xmlcraft_text::eol::LineEndings;
use xmlcraft_xsd::dom;

const XSI_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// What to assign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assignment<'a> {
    /// A DTD: `<!DOCTYPE root SYSTEM "location">`.
    Dtd { location: &'a str },
    /// An XML Schema for the namespace (`None`: for no namespace), found at `location`.
    Schema { namespace: Option<&'a str>, location: &'a str },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssignError {
    /// The document has errors that make it unclear where the root element and its attributes are.
    NotWellFormed,
    NoRoot,
    /// The document has a DOCTYPE, which is used before a schema: assigning a schema would change nothing it is checked by.
    HasDoctype,
    /// The location cannot be written there, and why.
    BadLocation(String),
}

impl fmt::Display for AssignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AssignError::NotWellFormed => write!(f, "The document is not well-formed; fix that first."),
            AssignError::NoRoot => write!(f, "The document has no root element."),
            AssignError::HasDoctype => {
                write!(f, "The document has a DOCTYPE, which is used before a schema. Assign the DTD instead, or remove the DOCTYPE.")
            }
            AssignError::BadLocation(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for AssignError {}

fn range_of(node: &SyntaxNode) -> Range<usize> {
    let r = node.text_range();
    usize::from(r.start())..usize::from(r.end())
}

/// `text` as the value of an attribute written in double quotes.
fn escape_attribute(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}

/// The location as it is written in a DOCTYPE: in the quotes it does not contain.
fn quoted(location: &str) -> Result<String, AssignError> {
    if location.chars().any(char::is_control) || location.is_empty() {
        return Err(AssignError::BadLocation("The location is empty or has a control character.".to_owned()));
    }
    match (location.contains('"'), location.contains('\'')) {
        (false, _) => Ok(format!("\"{location}\"")),
        (true, false) => Ok(format!("'{location}'")),
        (true, true) => Err(AssignError::BadLocation("The location has both kinds of quotes, which a DOCTYPE cannot hold.".to_owned())),
    }
}

/// The start tag of the root element: its range, the end of its name, and its attributes (name as written, the range of the whole
/// attribute and of the text between its quotes).
struct Root {
    name: String,
    name_end: usize,
    attributes: Vec<RootAttribute>,
    /// Where the element starts.
    start: usize,
}

struct RootAttribute {
    name: String,
    end: usize,
    value: Range<usize>,
}

fn root_of(syntax: &SyntaxNode, text: &str) -> Option<Root> {
    let element = syntax.children().find(|c| c.kind() == SyntaxKind::Element)?;
    let start_tag = element.children().find(|c| c.kind() == SyntaxKind::StartTag)?;
    let name = start_tag.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name)?;
    let name_range = usize::from(name.text_range().start())..usize::from(name.text_range().end());
    let mut attributes = Vec::new();
    for attribute in start_tag.children().filter(|c| c.kind() == SyntaxKind::Attribute) {
        let Some(attr_name) = attribute.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name) else {
            continue;
        };
        let Some(value) = attribute.children().find(|c| c.kind() == SyntaxKind::AttrValue) else { continue };
        let range = range_of(&value);
        let quoted = text
            .get(range.clone())
            .is_some_and(|t| t.len() >= 2 && (t.starts_with('"') || t.starts_with('\'')) && t.as_bytes().first() == t.as_bytes().last());
        let inner = if quoted { range.start + 1..range.end - 1 } else { range.clone() };
        attributes.push(RootAttribute { name: attr_name.text().to_owned(), end: range_of(&attribute).end, value: inner });
    }
    Some(Root { name: name.text().to_owned(), name_end: name_range.end, attributes, start: range_of(&element).start })
}

/// Whether a location names a DTD: it ends in `.dtd`, `.ent` or `.mod` (anything else is taken for an XML Schema).
pub fn is_dtd_location(location: &str) -> bool {
    let path = location.split(['?', '#']).next().unwrap_or(location);
    path.rsplit_once('.').is_some_and(|(_, ext)| ["dtd", "ent", "mod"].iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

/// Edits that assign `what` to the document `text`; none when it is assigned already, as it would be written.
pub fn assign(text: &str, what: &Assignment<'_>) -> Result<Vec<Edit>, AssignError> {
    let mut edits = edits_for(text, what)?;
    edits.retain(|e| text.get(e.range.clone()) != Some(e.text.as_str()));
    Ok(edits)
}

fn edits_for(text: &str, what: &Assignment<'_>) -> Result<Vec<Edit>, AssignError> {
    let parsed = parse(text);
    if parsed.diagnostics().iter().any(|d| d.severity() == Severity::Error) {
        return Err(AssignError::NotWellFormed);
    }
    let syntax = parsed.syntax();
    let root = root_of(&syntax, text).ok_or(AssignError::NoRoot)?;
    match what {
        Assignment::Dtd { location } => assign_dtd(text, &syntax, &root, location),
        Assignment::Schema { namespace, location } => {
            if doctype_parts(&syntax).is_some() {
                return Err(AssignError::HasDoctype);
            }
            assign_schema(text, &root, *namespace, location)
        }
    }
}

fn assign_dtd(text: &str, syntax: &SyntaxNode, root: &Root, location: &str) -> Result<Vec<Edit>, AssignError> {
    let literal = quoted(location)?;
    if let Some(parts) = doctype_parts(syntax) {
        // The DOCTYPE stays what it is, with its name and its internal subset; only what it says about an external subset changes.
        // A DOCTYPE without a name is not well-formed, which was refused above: an error is more honest than a name made up.
        let Some(name) = parts.name.clone() else { return Err(AssignError::NotWellFormed) };
        let subset = parts.subset.as_ref().and_then(|r| text.get(r.clone()));
        let new = match subset {
            Some(subset) => format!("<!DOCTYPE {name} SYSTEM {literal} [{subset}]>"),
            None => format!("<!DOCTYPE {name} SYSTEM {literal}>"),
        };
        return Ok(vec![Edit { range: parts.range, text: new }]);
    }
    // Before the root element, on a line of its own when the root element is on one.
    let before = text.get(..root.start).unwrap_or_default();
    let on_a_line_of_its_own = before.is_empty() || before.ends_with('\n') || before.ends_with('\r');
    let eol = if on_a_line_of_its_own { LineEndings::count(text).dominant().as_str() } else { "" };
    Ok(vec![Edit { range: root.start..root.start, text: format!("<!DOCTYPE {} SYSTEM {literal}>{eol}", root.name) }])
}

fn assign_schema(text: &str, root: &Root, namespace: Option<&str>, location: &str) -> Result<Vec<Edit>, AssignError> {
    if location.chars().any(|c| c.is_control() || c.is_whitespace()) || location.is_empty() {
        return Err(AssignError::BadLocation(
            "The location is empty, or has a space or a control character, which a schema location cannot hold.".to_owned(),
        ));
    }
    if namespace.is_some_and(|n| n.chars().any(char::is_whitespace) || n.is_empty()) {
        return Err(AssignError::BadLocation("The namespace is empty or has a space.".to_owned()));
    }
    // The prefix the schema-instance namespace has on the root element, or one that it can have.
    let declared = |prefix: &str| root.attributes.iter().find(|a| a.name == format!("xmlns:{prefix}"));
    let bound_to_xsi = root.attributes.iter().find_map(|a| {
        let prefix = a.name.strip_prefix("xmlns:")?;
        (text.get(a.value.clone()).map(decode_references).as_deref() == Some(XSI_NAMESPACE)).then(|| prefix.to_owned())
    });
    let mut added_declaration = false;
    let prefix = match bound_to_xsi {
        Some(prefix) => prefix,
        None => {
            added_declaration = true;
            let mut candidate = "xsi".to_owned();
            let mut n = 1usize;
            while declared(&candidate).is_some() {
                n += 1;
                candidate = format!("xsi{n}");
            }
            candidate
        }
    };
    let (attribute_name, wanted) = match namespace {
        None => (format!("{prefix}:noNamespaceSchemaLocation"), escape_attribute(location)),
        Some(ns) => (format!("{prefix}:schemaLocation"), format!("{} {}", escape_attribute(ns), escape_attribute(location))),
    };
    if let Some(existing) = root.attributes.iter().find(|a| a.name == attribute_name) {
        let current = text.get(existing.value.clone()).unwrap_or_default();
        return Ok(vec![match namespace {
            None => Edit { range: existing.value.clone(), text: escape_attribute(location) },
            Some(ns) => with_pair(current, existing.value.start, ns, location),
        }]);
    }
    let mut inserted = String::new();
    if added_declaration {
        inserted.push_str(&format!(" xmlns:{prefix}=\"{XSI_NAMESPACE}\""));
    }
    inserted.push_str(&format!(" {attribute_name}=\"{wanted}\""));
    let at = root.attributes.last().map_or(root.name_end, |a| a.end);
    Ok(vec![Edit { range: at..at, text: inserted }])
}

/// The edit that puts `location` for `namespace` into the value of `xsi:schemaLocation`, which is `current` (as written) and starts at
/// `origin` in the document: the location that is there replaced, or a pair added at the end. The other words, and the white space
/// between them, stay as they are.
fn with_pair(current: &str, origin: usize, namespace: &str, location: &str) -> Edit {
    // The words with where they are in the value.
    let mut words: Vec<(Range<usize>, String)> = Vec::new();
    let mut start = None;
    for (at, c) in current.char_indices().chain([(current.len(), ' ')]) {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(at),
            (true, Some(from)) => {
                words.push((from..at, decode_references(current.get(from..at).unwrap_or_default())));
                start = None;
            }
            _ => {}
        }
    }
    let namespace_at = words.chunks(2).position(|pair| pair.first().is_some_and(|(_, ns)| ns == namespace));
    if let Some((range, _)) = namespace_at.and_then(|pair| words.get(pair * 2 + 1)) {
        return Edit { range: origin + range.start..origin + range.end, text: escape_attribute(location) };
    }
    let Some((last, last_word)) = words.last() else {
        // An empty value: the pair is all there is.
        return Edit { range: origin..origin + current.len(), text: format!("{} {}", escape_attribute(namespace), escape_attribute(location)) };
    };
    let end = origin + last.end;
    // An odd number of words ends in a namespace: when it is this one, only its location is missing.
    if words.len() % 2 == 1 && last_word == namespace {
        return Edit { range: end..end, text: format!(" {}", escape_attribute(location)) };
    }
    Edit { range: end..end, text: format!(" {} {}", escape_attribute(namespace), escape_attribute(location)) }
}

/// The target namespace of the schema document `text`, when it is one and has one.
pub fn schema_namespace(text: &str) -> Option<String> {
    let parsed = parse(text);
    let tree = dom::build(&parsed.syntax());
    let root = tree.elements.first()?;
    if root.local != "schema" || root.namespace.as_deref() != Some(xmlcraft_xsd::XSD_NAMESPACE) {
        return None;
    }
    root.attrs.iter().find(|a| a.namespace.is_none() && a.local == "targetNamespace").map(|a| a.value.clone()).filter(|v| !v.is_empty())
}

/// How to write the place of `to` from the folder of the document `document`: relative, with `/`, when they are on the same root, else the
/// whole path (with `/`).
pub fn relative_location(document: &Path, to: &Path) -> String {
    let from: Vec<Component<'_>> = document.parent().map(|p| p.components().collect()).unwrap_or_default();
    let target: Vec<Component<'_>> = to.components().collect();
    let slash = |p: &Path| p.to_string_lossy().replace('\\', "/");
    let common = from.iter().zip(&target).take_while(|(a, b)| a == b).count();
    // Nothing in common (another drive, or a relative path against an absolute one): the path as it is.
    if common == 0 {
        return slash(to);
    }
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len().saturating_sub(common)];
    parts.extend(target.iter().skip(common).map(|c| c.as_os_str().to_string_lossy().into_owned()));
    if parts.is_empty() { slash(to) } else { parts.join("/") }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn apply(text: &str, edits: &[Edit]) -> String {
        let mut sorted = edits.to_vec();
        sorted.sort_by_key(|e| std::cmp::Reverse(e.range.start));
        let mut out = text.to_owned();
        for e in sorted {
            out.replace_range(e.range, &e.text);
        }
        out
    }

    fn assigned(text: &str, what: Assignment<'_>) -> String {
        apply(text, &assign(text, &what).unwrap_or_else(|e| panic!("{e}")))
    }

    #[test]
    fn a_dtd_is_a_doctype_before_the_root_on_a_line_of_its_own() {
        let dtd = Assignment::Dtd { location: "order.dtd" };
        assert_eq!(assigned("<order/>", dtd), "<!DOCTYPE order SYSTEM \"order.dtd\">\n<order/>");
        assert_eq!(
            assigned("<?xml version=\"1.0\"?>\n<!-- c -->\n<order>x</order>\n", dtd),
            "<?xml version=\"1.0\"?>\n<!-- c -->\n<!DOCTYPE order SYSTEM \"order.dtd\">\n<order>x</order>\n"
        );
        // The line endings of the document are the ones used.
        assert_eq!(
            assigned("<?xml version=\"1.0\"?>\r\n<a/>\r\n", Assignment::Dtd { location: "a.dtd" }),
            "<?xml version=\"1.0\"?>\r\n<!DOCTYPE a SYSTEM \"a.dtd\">\r\n<a/>\r\n"
        );
        // On the line of the declaration, it stays on it.
        assert_eq!(
            assigned("<?xml version=\"1.0\"?><a/>", Assignment::Dtd { location: "a.dtd" }),
            "<?xml version=\"1.0\"?><!DOCTYPE a SYSTEM \"a.dtd\"><a/>"
        );
        // A prefixed root name is written as it is.
        assert_eq!(
            assigned("<p:a xmlns:p=\"urn:p\"/>", Assignment::Dtd { location: "a.dtd" }),
            "<!DOCTYPE p:a SYSTEM \"a.dtd\">\n<p:a xmlns:p=\"urn:p\"/>"
        );
    }

    #[test]
    fn an_existing_doctype_keeps_its_name_and_its_internal_subset() {
        let before = "<!DOCTYPE a PUBLIC \"-//X//DTD//EN\" \"old.dtd\" [\n<!ENTITY e \"v\">\n]>\n<a>&e;</a>";
        let after = assigned(before, Assignment::Dtd { location: "new.dtd" });
        assert_eq!(after, "<!DOCTYPE a SYSTEM \"new.dtd\" [\n<!ENTITY e \"v\">\n]>\n<a>&e;</a>");
        assert_eq!(assigned("<!DOCTYPE a>\n<a/>", Assignment::Dtd { location: "n.dtd" }), "<!DOCTYPE a SYSTEM \"n.dtd\">\n<a/>");
    }

    #[test]
    fn a_location_with_a_quote_is_quoted_with_the_other_one() {
        assert_eq!(assigned("<a/>", Assignment::Dtd { location: "my\"file.dtd" }), "<!DOCTYPE a SYSTEM 'my\"file.dtd'>\n<a/>");
        assert!(matches!(assign("<a/>", &Assignment::Dtd { location: "a\"b'c.dtd" }), Err(AssignError::BadLocation(_))));
        assert!(matches!(assign("<a/>", &Assignment::Dtd { location: "" }), Err(AssignError::BadLocation(_))));
    }

    #[test]
    fn a_schema_for_no_namespace_is_an_attribute_and_the_declaration_of_its_prefix() {
        let schema = Assignment::Schema { namespace: None, location: "a.xsd" };
        assert_eq!(assigned("<a/>", schema), "<a xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"a.xsd\"/>");
        // After the last attribute, with the layout of the tag as it is.
        assert_eq!(
            assigned("<a id=\"1\"\n   b=\"2\">x</a>", schema),
            "<a id=\"1\"\n   b=\"2\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"a.xsd\">x</a>"
        );
        // The prefix the document already uses for the namespace is the one used, and not declared again.
        assert_eq!(
            assigned("<a xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\"/>", schema),
            "<a xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\" i:noNamespaceSchemaLocation=\"a.xsd\"/>"
        );
        // A prefix `xsi` that means something else is not taken.
        assert_eq!(
            assigned("<a xmlns:xsi=\"urn:other\"/>", schema),
            "<a xmlns:xsi=\"urn:other\" xmlns:xsi2=\"http://www.w3.org/2001/XMLSchema-instance\" xsi2:noNamespaceSchemaLocation=\"a.xsd\"/>"
        );
    }

    #[test]
    fn a_schema_that_is_assigned_already_is_changed_not_repeated() {
        let doc = "<a xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"old.xsd\"/>";
        assert_eq!(
            assigned(doc, Assignment::Schema { namespace: None, location: "new&better.xsd" }),
            "<a xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"new&amp;better.xsd\"/>"
        );
        let pairs = "<a xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"urn:a a.xsd urn:b b.xsd\"/>";
        assert_eq!(
            assigned(pairs, Assignment::Schema { namespace: Some("urn:b"), location: "new-b.xsd" }),
            "<a xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"urn:a a.xsd urn:b new-b.xsd\"/>"
        );
        assert_eq!(
            assigned(pairs, Assignment::Schema { namespace: Some("urn:c"), location: "c.xsd" }),
            "<a xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"urn:a a.xsd urn:b b.xsd urn:c c.xsd\"/>"
        );
    }

    #[test]
    fn the_pairs_of_a_schema_location_keep_their_layout_when_one_is_changed_or_added() {
        let xsi = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";
        let doc = format!("<a {xsi}\n   xsi:schemaLocation=\"urn:a   a.xsd\n                       urn:b\tb.xsd\"/>");
        let to = |ns: &str, location: &str| assigned(&doc, Assignment::Schema { namespace: Some(ns), location });
        // Only the word that is the location changes: the line break, the tab and the runs of spaces stay.
        assert_eq!(to("urn:b", "new.xsd"), doc.replace("b.xsd", "new.xsd"));
        assert_eq!(to("urn:a", "new.xsd"), doc.replace("a.xsd", "new.xsd"));
        // A location is not taken for a namespace: `a.xsd` stands in the place of a namespace only when it is one.
        assert_eq!(to("a.xsd", "x.xsd"), doc.replace("b.xsd\"", "b.xsd a.xsd x.xsd\""));
        // A new pair is appended after the last word.
        assert_eq!(to("urn:c", "c.xsd"), doc.replace("b.xsd\"", "b.xsd urn:c c.xsd\""));
        // A namespace that is there without a location gets one, and an empty value gets the pair.
        let lonely = format!("<a {xsi} xsi:schemaLocation=\"urn:a\"/>");
        assert_eq!(assigned(&lonely, Assignment::Schema { namespace: Some("urn:a"), location: "a.xsd" }), lonely.replace("urn:a\"", "urn:a a.xsd\""));
        let empty = format!("<a {xsi} xsi:schemaLocation=\"\"/>");
        assert_eq!(assigned(&empty, Assignment::Schema { namespace: Some("urn:a"), location: "a.xsd" }), empty.replace("=\"\"", "=\"urn:a a.xsd\""));
        // A namespace written with a reference is found as what it means.
        let reference = format!("<a {xsi} xsi:schemaLocation=\"urn:&#97; a.xsd\"/>");
        assert_eq!(assigned(&reference, Assignment::Schema { namespace: Some("urn:a"), location: "b.xsd" }), reference.replace("a.xsd", "b.xsd"));
    }

    #[test]
    fn a_schema_for_a_namespace_is_a_pair() {
        assert_eq!(
            assigned("<p:a xmlns:p=\"urn:p\"/>", Assignment::Schema { namespace: Some("urn:p"), location: "p.xsd" }),
            "<p:a xmlns:p=\"urn:p\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"urn:p p.xsd\"/>"
        );
    }

    #[test]
    fn what_cannot_be_assigned_is_refused_and_says_why() {
        let schema = Assignment::Schema { namespace: None, location: "a.xsd" };
        assert_eq!(assign("<a><b></a>", &schema), Err(AssignError::NotWellFormed));
        assert_eq!(assign("<!-- nothing -->", &schema), Err(AssignError::NotWellFormed), "a document with no root element is not well-formed");
        assert_eq!(assign("<!DOCTYPE a>\n<a/>", &schema), Err(AssignError::HasDoctype));
        assert!(matches!(assign("<a/>", &Assignment::Schema { namespace: None, location: "my schema.xsd" }), Err(AssignError::BadLocation(_))));
        assert!(matches!(assign("<a/>", &Assignment::Schema { namespace: Some(""), location: "a.xsd" }), Err(AssignError::BadLocation(_))));
    }

    #[test]
    fn only_what_is_assigned_changes() {
        let doc = "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\n<!-- keep -->\n<a  x = \"1\" >\n\t<b/>\n</a>\n";
        for what in [Assignment::Dtd { location: "a.dtd" }, Assignment::Schema { namespace: None, location: "a.xsd" }] {
            let edits = assign(doc, &what).expect("assigned");
            assert_eq!(edits.len(), 1);
            let changed = apply(doc, &edits);
            // Everything that was there is still there, in order.
            let (head, tail) = (&doc[..edits[0].range.start], &doc[edits[0].range.end..]);
            assert!(changed.starts_with(head) && changed.ends_with(tail), "{changed}");
        }
    }

    /// What a document is checked against once something is assigned: the schema or the DTD is found by the name it was assigned under.
    #[test]
    fn what_is_assigned_is_what_the_document_is_then_checked_against() {
        use xmlcraft_dtd::{LoadRequest, Loaded, SubsetLoader};
        use xmlcraft_xsd::{LoadRequest as SchemaRequest, Loaded as SchemaLoaded, SchemaLoader};

        use crate::validation::{Verdict, validate};

        struct Dtd;
        impl SubsetLoader for Dtd {
            fn load(&mut self, r: &LoadRequest<'_>) -> Result<Loaded, String> {
                (r.system == "../dtds/order.dtd")
                    .then(|| Loaded {
                        text: "<!ELEMENT order (item+)>\n<!ELEMENT item (#PCDATA)>\n".to_owned(),
                        location: "mem:order.dtd".to_owned(),
                    })
                    .ok_or_else(|| format!("{}: not there", r.system))
            }
        }
        struct Xsd(&'static str);
        impl SchemaLoader for Xsd {
            fn load(&mut self, r: &SchemaRequest<'_>) -> Result<SchemaLoaded, String> {
                let wanted = r.location.unwrap_or_default();
                let text = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" targetNamespace=\"urn:o\" elementFormDefault=\"qualified\"><xs:element name=\"order\"><xs:complexType><xs:sequence><xs:element name=\"item\" type=\"xs:string\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType></xs:element></xs:schema>";
                (wanted == self.0)
                    .then(|| SchemaLoaded { text: text.to_owned(), location: format!("mem:{wanted}") })
                    .ok_or_else(|| format!("{wanted}: not there"))
            }
        }
        let dtd_doc = "<?xml version=\"1.0\"?>\n<order><item>a</item></order>\n";
        let with_dtd = assigned(dtd_doc, Assignment::Dtd { location: "../dtds/order.dtd" });
        let made = validate(&with_dtd, 1, "/d/order.xml", &mut Dtd, &mut Xsd(""));
        assert_eq!((made.verdict, made.problems.len()), (Verdict::Valid, 0), "{with_dtd}");
        // Another DTD of the same document: still a document that is checked, and found wanting.
        let wrong = assigned("<order><thing/></order>", Assignment::Dtd { location: "../dtds/order.dtd" });
        assert_eq!(validate(&wrong, 1, "/d/order.xml", &mut Dtd, &mut Xsd("")).verdict, Verdict::Invalid);
        let xsd_doc = "<order xmlns=\"urn:o\"><item>a</item></order>";
        let with_xsd = assigned(xsd_doc, Assignment::Schema { namespace: Some("urn:o"), location: "order.xsd" });
        let made = validate(&with_xsd, 1, "/d/order.xml", &mut Dtd, &mut Xsd("order.xsd"));
        assert_eq!((made.verdict, made.problems.len()), (Verdict::Valid, 0), "{with_xsd}");
        let bad = assigned("<order xmlns=\"urn:o\"><nope/></order>", Assignment::Schema { namespace: Some("urn:o"), location: "order.xsd" });
        assert_eq!(validate(&bad, 1, "/d/order.xml", &mut Dtd, &mut Xsd("order.xsd")).verdict, Verdict::Invalid);
    }

    #[test]
    fn a_dtd_is_told_by_the_end_of_its_name() {
        for dtd in ["a.dtd", "a.DTD", "../x/y.ent", "m.mod", "http://x.org/a.dtd", "http://x.org/a.dtd?v=2"] {
            assert!(is_dtd_location(dtd), "{dtd}");
        }
        for other in ["a.xsd", "a.xml", "dtd", "a.dtd.xsd", "", "dir.dtd/file"] {
            assert!(!is_dtd_location(other), "{other}");
        }
    }

    #[test]
    fn the_target_namespace_of_a_schema_document() {
        assert_eq!(
            schema_namespace("<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" targetNamespace=\"urn:t\"/>").as_deref(),
            Some("urn:t")
        );
        assert_eq!(schema_namespace("<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"/>"), None);
        assert_eq!(schema_namespace("<schema targetNamespace=\"urn:t\"/>"), None, "a schema element of no namespace is not a schema");
        assert_eq!(schema_namespace("<!ELEMENT a EMPTY>"), None);
    }

    #[test]
    fn what_is_assigned_already_needs_no_edit() {
        for what in [
            Assignment::Dtd { location: "a.dtd" },
            Assignment::Schema { namespace: None, location: "a.xsd" },
            Assignment::Schema { namespace: Some("urn:a"), location: "a.xsd" },
        ] {
            let once = assigned("<a xmlns=\"urn:a\">x</a>", what);
            assert_eq!(assign(&once, &what), Ok(Vec::new()), "{once}");
        }
        // Another location is an edit.
        let once = assigned("<a/>", Assignment::Dtd { location: "a.dtd" });
        assert_eq!(assign(&once, &Assignment::Dtd { location: "b.dtd" }).map(|e| e.len()), Ok(1));
    }

    #[test]
    fn a_root_after_a_lone_carriage_return_is_on_a_line_of_its_own() {
        assert_eq!(assigned("<!-- c -->\r<a/>", Assignment::Dtd { location: "a.dtd" }), "<!-- c -->\r<!DOCTYPE a SYSTEM \"a.dtd\">\r<a/>");
    }

    #[test]
    fn the_prefix_of_the_instance_namespace_is_found_by_what_it_means_not_by_how_it_is_written() {
        // `&#104;` is an `h`.
        let doc = "<a xmlns:i=\"&#104;ttp://www.w3.org/2001/XMLSchema-instance\"/>";
        assert_eq!(
            assigned(doc, Assignment::Schema { namespace: None, location: "a.xsd" }),
            "<a xmlns:i=\"&#104;ttp://www.w3.org/2001/XMLSchema-instance\" i:noNamespaceSchemaLocation=\"a.xsd\"/>"
        );
    }

    #[test]
    fn a_namespace_that_cannot_be_written_is_refused() {
        for namespace in ["", "urn: a", "urn:a\n"] {
            assert!(
                matches!(assign("<a/>", &Assignment::Schema { namespace: Some(namespace), location: "a.xsd" }), Err(AssignError::BadLocation(_))),
                "{namespace:?}"
            );
        }
    }

    #[test]
    fn the_target_namespace_is_that_of_a_schema_that_has_one() {
        let xs = "xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"";
        // Not the root of a schema: an element of another name in the schema namespace, or a `schema` in none.
        assert_eq!(schema_namespace(&format!("<xs:other {xs} targetNamespace=\"urn:x\"/>")), None);
        assert_eq!(schema_namespace("<schema targetNamespace=\"urn:x\"/>"), None);
        // An empty target namespace is none.
        assert_eq!(schema_namespace(&format!("<xs:schema {xs} targetNamespace=\"\"/>")), None);
        assert_eq!(schema_namespace(&format!("<xs:schema {xs} targetNamespace=\"urn:x\"/>")).as_deref(), Some("urn:x"));
    }

    #[test]
    fn a_location_is_written_from_the_folder_of_the_document() {
        let doc = PathBuf::from(if cfg!(windows) { "C:\\work\\docs\\a.xml" } else { "/work/docs/a.xml" });
        let p = |s: &str| PathBuf::from(if cfg!(windows) { format!("C:{}", s.replace('/', "\\")) } else { s.to_owned() });
        assert_eq!(relative_location(&doc, &p("/work/docs/a.xsd")), "a.xsd");
        assert_eq!(relative_location(&doc, &p("/work/docs/sub/a.xsd")), "sub/a.xsd");
        assert_eq!(relative_location(&doc, &p("/work/schemas/a.xsd")), "../schemas/a.xsd");
        assert_eq!(relative_location(&doc, &p("/other/a.xsd")), "../../other/a.xsd");
        // A path with nothing in common with the folder (here: relative) is written as it is.
        assert_eq!(relative_location(&doc, Path::new("schemas/a.xsd")), "schemas/a.xsd");
    }
}
