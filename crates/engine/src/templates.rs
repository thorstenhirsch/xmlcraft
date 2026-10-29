//! Templates for a new document: a small start for each kind of file XmlCraft works with. The text is original (nothing is taken from
//! another product); each is what a person would type first, so that the Entry Helpers and the validation have something to work with.

/// A kind of document to start from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    /// A short name that does not change (for commands and tests).
    pub id: &'static str,
    pub title: &'static str,
    /// What it is for, in a sentence.
    pub description: &'static str,
    /// The name a document made from it is shown and saved under by default.
    pub name: &'static str,
    pub text: &'static str,
}

/// Every template, in the order they are offered.
pub const TEMPLATES: &[Template] = &[
    Template {
        id: "xml",
        title: "XML document",
        description: "A document with an XML declaration and a root element.",
        name: "document.xml",
        text: "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<root>\n  \n</root>\n",
    },
    Template {
        id: "xml-dtd",
        title: "XML document with a DTD",
        description: "A document that declares its own elements in an internal DTD, so that it can be validated at once.",
        name: "document.xml",
        text: "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE root [\n  <!ELEMENT root (item*)>\n  <!ELEMENT item (#PCDATA)>\n  <!ATTLIST item id ID #IMPLIED>\n]>\n<root>\n  <item id=\"first\">One</item>\n</root>\n",
    },
    Template {
        id: "xsd",
        title: "XML Schema",
        description: "A schema (XSD 1.0) with a target namespace and one element, qualified, ready to grow.",
        name: "schema.xsd",
        text: "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"\n           targetNamespace=\"urn:example:document\"\n           xmlns=\"urn:example:document\"\n           elementFormDefault=\"qualified\">\n  <xs:element name=\"document\">\n    <xs:complexType>\n      <xs:sequence>\n        <xs:element name=\"title\" type=\"xs:string\"/>\n      </xs:sequence>\n      <xs:attribute name=\"id\" type=\"xs:ID\" use=\"required\"/>\n    </xs:complexType>\n  </xs:element>\n</xs:schema>\n",
    },
    Template {
        id: "dtd",
        title: "DTD",
        description: "A document type definition with a root element, a repeated child and an attribute.",
        name: "document.dtd",
        text: "<!ELEMENT root (item*)>\n<!ELEMENT item (#PCDATA)>\n<!ATTLIST item\n  id ID #IMPLIED>\n",
    },
    Template {
        id: "xhtml",
        title: "XHTML 1.0 Strict page",
        description: "A page that names the XHTML 1.0 Strict DTD by its public identifier (the app has the DTD, so it validates offline).",
        name: "page.xhtml",
        text: "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd\">\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"en\" lang=\"en\">\n  <head>\n    <title>Title</title>\n  </head>\n  <body>\n    <p>Text.</p>\n  </body>\n</html>\n",
    },
    Template {
        id: "xslt",
        title: "XSLT 1.0 stylesheet",
        description: "A stylesheet that copies its input and has a place for the templates that change it.",
        name: "stylesheet.xsl",
        text: "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xsl:stylesheet version=\"1.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">\n  <xsl:output method=\"xml\" indent=\"yes\"/>\n\n  <xsl:template match=\"@*|node()\">\n    <xsl:copy>\n      <xsl:apply-templates select=\"@*|node()\"/>\n    </xsl:copy>\n  </xsl:template>\n</xsl:stylesheet>\n",
    },
    Template {
        id: "catalog",
        title: "OASIS XML catalog",
        description: "A catalog that maps the identifier of a schema or a DTD to a file of your own.",
        name: "catalog.xml",
        text: "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<catalog xmlns=\"urn:oasis:names:tc:entity:xmlns:xml:catalog\">\n  <system systemId=\"http://example.org/schema.xsd\" uri=\"schema.xsd\"/>\n</catalog>\n",
    },
];

/// The template with this id.
pub fn template(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

#[cfg(test)]
mod tests {
    use xmlcraft_syntax::{Severity, parse};

    use super::*;

    #[test]
    fn the_ids_are_unique_and_every_template_says_what_it_is() {
        for (i, t) in TEMPLATES.iter().enumerate() {
            assert!(!t.id.is_empty() && !t.title.is_empty() && !t.description.is_empty() && !t.name.is_empty() && !t.text.is_empty(), "{t:?}");
            assert!(TEMPLATES.iter().skip(i + 1).all(|other| other.id != t.id), "two templates are called {}", t.id);
            assert!(t.text.ends_with('\n') && !t.text.contains('\r'), "{}: the text ends its last line, and has line feeds only", t.id);
            assert_eq!(template(t.id), Some(t));
        }
        assert_eq!(template("nope"), None);
    }

    #[test]
    fn every_template_that_is_xml_is_well_formed() {
        for t in TEMPLATES.iter().filter(|t| !t.name.ends_with(".dtd")) {
            let parsed = parse(t.text);
            let errors: Vec<_> = parsed.diagnostics().iter().filter(|d| d.severity() == Severity::Error).map(|d| d.message()).collect();
            assert!(errors.is_empty(), "{}: {errors:?}", t.id);
        }
    }

    #[test]
    fn the_extension_of_a_name_says_what_the_template_is() {
        for t in TEMPLATES {
            let extension = t.name.rsplit_once('.').map(|(_, e)| e);
            let expected: &[&str] = match t.id {
                "xml" | "xml-dtd" | "catalog" => &["xml"],
                "xsd" => &["xsd"],
                "dtd" => &["dtd"],
                "xhtml" => &["xhtml"],
                "xslt" => &["xsl", "xslt"],
                other => panic!("a template this test does not know: {other}"),
            };
            assert!(extension.is_some_and(|e| expected.contains(&e)), "{}: {}", t.id, t.name);
        }
    }
}
