//! The schema or DTD a document names: what Load Schema opens.

use crate::analysis::{Analysis, DoctypeIds};

/// What a document names as its grammar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaLink {
    /// The DTD of a DOCTYPE: its public and its system identifier, as written.
    ExternalId { public: Option<String>, system: Option<String> },
    /// A schema location of the root element (`xsi:schemaLocation`, `xsi:noNamespaceSchemaLocation`), as written, and the namespace it
    /// is for.
    Uri { location: String, namespace: Option<String> },
}

impl SchemaLink {
    /// What to call it: the file name of its location, else its public identifier, else its namespace.
    pub fn label(&self) -> String {
        let file = |s: &str| s.rsplit(['/', '\\']).next().filter(|f| !f.is_empty()).map(str::to_owned);
        let label = match self {
            SchemaLink::ExternalId { public, system } => system.as_deref().and_then(file).or_else(|| public.clone()),
            SchemaLink::Uri { location, namespace } => file(location).or_else(|| namespace.clone()),
        };
        label.filter(|l| !l.is_empty()).unwrap_or_else(|| "schema".to_owned())
    }
}

/// The grammar the document of `analysis` names: its DOCTYPE when that has an external identifier (a DOCTYPE with only an internal
/// subset names nothing), else the first schema location of its root element. A namespace alone is not a link: the app may know a
/// schema for it, but the document does not name one.
pub fn schema_link(analysis: &Analysis) -> Option<SchemaLink> {
    let nonempty = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.trim().is_empty());
    if let Some(DoctypeIds { public, system }) = &analysis.doctype_ids
        && (nonempty(public) || nonempty(system))
    {
        return Some(SchemaLink::ExternalId {
            public: public.clone().filter(|_| nonempty(public)),
            system: system.clone().filter(|_| nonempty(system)),
        });
    }
    analysis
        .schema_hints
        .iter()
        .find(|hint| !hint.location.trim().is_empty())
        .map(|hint| SchemaLink::Uri { location: hint.location.clone(), namespace: hint.namespace.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    fn link(text: &str) -> Option<SchemaLink> {
        schema_link(&analyze(text, 0))
    }

    #[test]
    fn a_doctype_with_an_identifier_names_a_dtd() {
        assert_eq!(link("<!DOCTYPE r SYSTEM \"r.dtd\"><r/>"), Some(SchemaLink::ExternalId { public: None, system: Some("r.dtd".into()) }));
        assert_eq!(
            link("<!DOCTYPE r PUBLIC \"-//X//DTD R//EN\" \"http://x/r.dtd\"><r/>"),
            Some(SchemaLink::ExternalId { public: Some("-//X//DTD R//EN".into()), system: Some("http://x/r.dtd".into()) })
        );
        assert_eq!(link("<!DOCTYPE r PUBLIC \"-//X//DTD R//EN\"><r/>").map(|l| l.label()), Some("-//X//DTD R//EN".to_owned()));
    }

    #[test]
    fn an_internal_subset_alone_or_no_doctype_names_nothing() {
        assert_eq!(link("<!DOCTYPE r [<!ELEMENT r EMPTY>]><r/>"), None);
        assert_eq!(link("<r/>"), None);
        assert_eq!(link(""), None);
    }

    #[test]
    fn a_schema_location_names_a_schema_and_the_doctype_comes_first() {
        let xsi = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";
        assert_eq!(
            link(&format!("<r {xsi} xsi:noNamespaceSchemaLocation=\"s/r.xsd\"/>")),
            Some(SchemaLink::Uri { location: "s/r.xsd".into(), namespace: None })
        );
        assert_eq!(
            link(&format!("<r {xsi} xsi:schemaLocation=\"urn:r http://x/r.xsd\"/>")),
            Some(SchemaLink::Uri { location: "http://x/r.xsd".into(), namespace: Some("urn:r".into()) })
        );
        assert!(matches!(
            link(&format!("<!DOCTYPE r SYSTEM \"r.dtd\"><r {xsi} xsi:noNamespaceSchemaLocation=\"r.xsd\"/>")),
            Some(SchemaLink::ExternalId { .. })
        ));
    }

    #[test]
    fn a_namespace_alone_is_not_a_link() {
        assert_eq!(link("<r xmlns=\"http://www.w3.org/2001/XMLSchema\"/>"), None);
        assert_eq!(link("<r xmlns=\"urn:r\"/>"), None);
    }

    #[test]
    fn a_link_is_labelled_by_its_file() {
        assert_eq!(SchemaLink::Uri { location: "http://x/s/r.xsd".into(), namespace: None }.label(), "r.xsd");
        assert_eq!(SchemaLink::Uri { location: "".into(), namespace: Some("urn:r".into()) }.label(), "urn:r");
        assert_eq!(SchemaLink::Uri { location: "http://x/s/".into(), namespace: None }.label(), "schema", "a name is made up when there is none");
        assert_eq!(SchemaLink::ExternalId { public: None, system: Some("a\\b\\r.dtd".into()) }.label(), "r.dtd");
    }
}
