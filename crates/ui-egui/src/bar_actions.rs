//! The actions the command bar offers for the document being shown, beyond Validate and Format: one table, keyed by what the document
//! is ([`BarKind`]). What it is comes from [`DocKind`] and cheap looks at the text (the XSLT namespace on the root, the WSDL namespace,
//! an OpenAPI description that reads), worked out once the text has stood still ([`crate::business_chip::Settled`]), never per frame.

use xmlcraft_engine::doc_kind::DocKind;

use crate::XmlCraftApp;

/// What the command bar takes a document for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BarKind {
    /// An XML document that is none of the kinds below.
    Xml,
    /// An XML Schema or a DTD.
    Grammar,
    /// An XSLT stylesheet.
    Stylesheet,
    /// A WSDL description.
    Wsdl,
    /// JSON (JSONC, JSON5, JSON Lines, BSON) or YAML.
    Data,
    /// An OpenAPI description, in JSON or YAML.
    OpenApi,
    Html,
}

/// One action of the command bar after the fixed ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KindAction {
    /// A registry command, with the short label the bar gives it (the tooltip names the command and its shortcut).
    Command { id: &'static str, label: &'static str },
    /// The Schema split pill: a menu of these commands.
    SchemaMenu(&'static [&'static str]),
}

const XPATH: KindAction = KindAction::Command { id: "xml.xpath", label: "XPath" };
const XML_SCHEMA_MENU: KindAction = KindAction::SchemaMenu(&["xml.assign_schema", "xml.generate_schema", "xml.load_schema", "xml.schema_resolution"]);
const DATA_SCHEMA_MENU: KindAction = KindAction::SchemaMenu(&["xml.assign_schema", "file.generate_schema", "file.generate_sample"]);
const CONVERT: KindAction = KindAction::Command { id: "file.convert", label: "Convert" };

/// The actions for a kind of document, left to right.
pub(crate) fn kind_actions(kind: BarKind) -> &'static [KindAction] {
    match kind {
        BarKind::Xml => &[XML_SCHEMA_MENU, XPATH],
        BarKind::Grammar => {
            &[KindAction::Command { id: "xml.generate_sample", label: "Sample XML" }, KindAction::Command { id: "xml.check", label: "Check" }]
        }
        BarKind::Stylesheet => &[KindAction::Command { id: "xml.transform", label: "Run" }, XPATH],
        BarKind::Wsdl => &[KindAction::Command { id: "xml.soap", label: "SOAP Request" }],
        BarKind::Data => &[DATA_SCHEMA_MENU, CONVERT],
        BarKind::OpenApi => &[KindAction::Command { id: "xml.openapi", label: "Try Operation" }, DATA_SCHEMA_MENU, CONVERT],
        BarKind::Html => &[],
    }
}

/// What a look at the text found, beyond its [`DocKind`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Traits {
    pub stylesheet: bool,
    pub wsdl: bool,
    pub openapi: bool,
}

/// What the bar takes a document of `kind` with `traits` for.
pub(crate) fn bar_kind(kind: DocKind, traits: Traits) -> BarKind {
    match kind {
        DocKind::Xml if traits.stylesheet => BarKind::Stylesheet,
        DocKind::Xml if traits.wsdl => BarKind::Wsdl,
        DocKind::Xml => BarKind::Xml,
        DocKind::Xsd | DocKind::Dtd => BarKind::Grammar,
        DocKind::Json | DocKind::Yaml if traits.openapi => BarKind::OpenApi,
        DocKind::Json | DocKind::Yaml => BarKind::Data,
        DocKind::Html => BarKind::Html,
    }
}

impl XmlCraftApp {
    /// What the command bar takes the document being shown for, if one is.
    pub(crate) fn active_bar_kind(&mut self, ctx: &egui::Context) -> Option<BarKind> {
        let kind = self.active_doc_kind()?;
        let root_namespace = self.active_analysis().and_then(|a| a.root_namespace.clone()).filter(|_| kind == DocKind::Xml);
        let doc = self.session.active_document()?;
        let (id, version, len, language) = (doc.id(), doc.editor().version(), doc.editor().buffer().len(), doc.language());
        let view = self.views.get_mut(&id)?;
        let traits = view.bar_traits.get(ctx, version, || {
            let small = len <= crate::business_chip::MAX_BYTES;
            match kind {
                // (Only the start of the text is read for the XSLT namespace.)
                DocKind::Xml => {
                    Traits { stylesheet: xmlcraft_engine::xslt_complete::xslt_prefix(&doc.editor().text()).is_some(), ..Traits::default() }
                }
                DocKind::Json | DocKind::Yaml if small && (language.is_json_like() || language.is_yaml()) => Traits {
                    openapi: xmlcraft_engine::rest::describe(&doc.editor().text(), language.is_yaml()).is_ok_and(|d| !d.operations.is_empty()),
                    ..Traits::default()
                },
                _ => Traits::default(),
            }
        });
        // The root namespace is the analysis's, which arrives after the text: looked at every time (a comparison), not kept.
        let traits = Traits { wsdl: root_namespace.as_deref().is_some_and(xmlcraft_engine::soap::is_wsdl_namespace), ..traits };
        Some(bar_kind(kind, traits))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The command ids a kind gets, a Schema menu written as `[…]`.
    fn ids(kind: BarKind) -> Vec<String> {
        kind_actions(kind)
            .iter()
            .map(|a| match a {
                KindAction::Command { id, .. } => (*id).to_owned(),
                KindAction::SchemaMenu(ids) => format!("[{}]", ids.join(" ")),
            })
            .collect()
    }

    #[test]
    fn each_kind_of_document_gets_its_actions() {
        assert_eq!(ids(BarKind::Xml), ["[xml.assign_schema xml.generate_schema xml.load_schema xml.schema_resolution]", "xml.xpath"]);
        assert_eq!(ids(BarKind::Grammar), ["xml.generate_sample", "xml.check"]);
        assert_eq!(ids(BarKind::Stylesheet), ["xml.transform", "xml.xpath"]);
        assert_eq!(ids(BarKind::Wsdl), ["xml.soap"]);
        assert_eq!(ids(BarKind::Data), ["[xml.assign_schema file.generate_schema file.generate_sample]", "file.convert"]);
        assert_eq!(ids(BarKind::OpenApi), ["xml.openapi", "[xml.assign_schema file.generate_schema file.generate_sample]", "file.convert"]);
        assert!(ids(BarKind::Html).is_empty());
    }

    #[test]
    fn every_action_is_a_registry_command_so_the_palette_has_it_too() {
        for kind in [BarKind::Xml, BarKind::Grammar, BarKind::Stylesheet, BarKind::Wsdl, BarKind::Data, BarKind::OpenApi, BarKind::Html] {
            for action in kind_actions(kind) {
                let ids: &[&str] = match action {
                    KindAction::Command { id, .. } => std::slice::from_ref(id),
                    KindAction::SchemaMenu(ids) => ids,
                };
                for id in ids {
                    assert!(xmlcraft_engine::commands::command(id).is_some(), "{kind:?}: {id}");
                }
            }
        }
    }

    #[test]
    fn what_the_text_is_refines_the_kind_of_document() {
        let none = Traits::default();
        assert_eq!(bar_kind(DocKind::Xml, none), BarKind::Xml);
        assert_eq!(bar_kind(DocKind::Xml, Traits { stylesheet: true, ..none }), BarKind::Stylesheet);
        assert_eq!(bar_kind(DocKind::Xml, Traits { wsdl: true, ..none }), BarKind::Wsdl);
        assert_eq!(bar_kind(DocKind::Xsd, Traits { stylesheet: true, ..none }), BarKind::Grammar, "a schema stays a schema");
        assert_eq!(bar_kind(DocKind::Dtd, none), BarKind::Grammar);
        assert_eq!(bar_kind(DocKind::Json, none), BarKind::Data);
        assert_eq!(bar_kind(DocKind::Yaml, Traits { openapi: true, ..none }), BarKind::OpenApi);
        assert_eq!(bar_kind(DocKind::Xml, Traits { openapi: true, ..none }), BarKind::Xml, "OpenAPI is JSON or YAML");
        assert_eq!(bar_kind(DocKind::Html, none), BarKind::Html);
    }
}
