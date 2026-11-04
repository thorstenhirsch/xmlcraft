//! What a document is: an XML document, an XML Schema or a DTD. The command bar offers Schema… for the first and XML… for the others.

use xmlcraft_xsd::XSD_NAMESPACE;

use crate::analysis::Analysis;

/// What a document is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind {
    Xml,
    Xsd,
    Dtd,
}

/// What the document called `name` is: by its extension first (`.xsd` is a schema; `.dtd`, `.ent` and `.mod` are DTDs), else by its
/// text (a root element `schema` of the XML Schema namespace is a schema), else XML. `analysis` is the analysis of its text, when there
/// is one; without it only the name decides.
pub fn doc_kind(name: &str, analysis: Option<&Analysis>) -> DocKind {
    let extension = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase());
    match extension.as_deref() {
        Some("xsd") => return DocKind::Xsd,
        Some("dtd" | "ent" | "mod") => return DocKind::Dtd,
        _ => {}
    }
    let is_schema = analysis.is_some_and(|a| {
        a.root_namespace.as_deref() == Some(XSD_NAMESPACE) && a.outline.first().is_some_and(|root| root.name.rsplit(':').next() == Some("schema"))
    });
    if is_schema { DocKind::Xsd } else { DocKind::Xml }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    const XS: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"/>";

    #[test]
    fn the_extension_decides_first_and_in_any_case() {
        assert_eq!(doc_kind("a.xsd", None), DocKind::Xsd);
        assert_eq!(doc_kind("A.XSD", None), DocKind::Xsd);
        for name in ["a.dtd", "b.ent", "c.MOD"] {
            assert_eq!(doc_kind(name, None), DocKind::Dtd, "{name}");
        }
        assert_eq!(doc_kind("a.xml", None), DocKind::Xml);
        assert_eq!(doc_kind("Untitled-1", None), DocKind::Xml);
        assert_eq!(doc_kind("", None), DocKind::Xml);
        // The extension wins over what the text is.
        assert_eq!(doc_kind("a.dtd", Some(&analyze(XS, 0))), DocKind::Dtd);
        assert_eq!(doc_kind("a.xsd", Some(&analyze("<r/>", 0))), DocKind::Xsd);
    }

    #[test]
    fn a_schema_without_a_schema_extension_is_known_by_its_root() {
        assert_eq!(doc_kind("Untitled-1", Some(&analyze(XS, 0))), DocKind::Xsd);
        assert_eq!(doc_kind("x.xml", Some(&analyze("<schema xmlns=\"http://www.w3.org/2001/XMLSchema\"/>", 0))), DocKind::Xsd, "default namespace");
        // The same name in another namespace, or no namespace, is not a schema; nor is a schema that is not the root.
        assert_eq!(doc_kind("x.xml", Some(&analyze("<schema/>", 0))), DocKind::Xml);
        assert_eq!(doc_kind("x.xml", Some(&analyze("<schema xmlns=\"urn:other\"/>", 0))), DocKind::Xml);
        assert_eq!(
            doc_kind("x.xml", Some(&analyze("<wrapper xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:schema/></wrapper>", 0))),
            DocKind::Xml
        );
    }
}
