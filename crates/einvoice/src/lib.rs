#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! Recognising electronic invoices: which syntax a document is written in (UBL, UN/CEFACT Cross Industry Invoice, FatturaPA), which
//! kind of document it is, and which standard or profile it claims to follow (XRechnung, ZUGFeRD, Factur-X, Peppol BIS Billing 3,
//! EN 16931, …), from the root element and the identifiers the document carries.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web.
//! - Recognition is by what the document *says*: its root element and namespace, the customization or guideline identifier, and the
//!   version attributes; whether a document keeps the rules of the standard it names is for [`validate`].
//! - A document that is not one of the known syntaxes is `None`, never an error: it is just XML.
//! - [`validate::validate`] checks a recognised invoice stage by stage against the schemas and rules that ship with XmlCraft (the XML
//!   Schema of its syntax, the EN 16931 rules, the XRechnung rules).

pub mod model;
pub mod validate;

use xmlcraft_syntax::parse;
use xmlcraft_xsd::dom::{self, Dom, El};

const UBL_INVOICE: &str = "urn:oasis:names:specification:ubl:schema:xsd:Invoice-2";
const UBL_CREDIT_NOTE: &str = "urn:oasis:names:specification:ubl:schema:xsd:CreditNote-2";
const UBL_ORDER: &str = "urn:oasis:names:specification:ubl:schema:xsd:Order-2";
const UBL_ORDER_RESPONSE: &str = "urn:oasis:names:specification:ubl:schema:xsd:OrderResponse-2";
const UBL_DESPATCH_ADVICE: &str = "urn:oasis:names:specification:ubl:schema:xsd:DespatchAdvice-2";
const UBL_APPLICATION_RESPONSE: &str = "urn:oasis:names:specification:ubl:schema:xsd:ApplicationResponse-2";
const UBL_CBC: &str = "urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2";
const CII: &str = "urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100";
const CII_RAM: &str = "urn:un:unece:uncefact:data:standard:ReusableAggregateBusinessInformationEntity:100";
/// ZUGFeRD 1.0's root, `CrossIndustryDocument`, in its own namespace.
const ZUGFERD_1: &str = "urn:ferd:CrossIndustryDocument:invoice:1p0";
const ORDER_X: &str = "urn:un:unece:uncefact:data:standard:SCRDMCCBDACIOMessageStructure:100";
const FATTURAPA_PREFIX: &str = "http://ivaservizi.agenziaentrate.gov.it/docs/xsd/fatture/v";
const FATTURAPA_PREFIX_ALT: &str = "http://www.fatturapa.gov.it/export/fatturazione/sdi/fatturapa/v";

/// The XML syntax a document is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Syntax {
    /// OASIS Universal Business Language 2.x.
    Ubl,
    /// UN/CEFACT Cross Industry Invoice D16B (the syntax of ZUGFeRD 2, Factur-X and XRechnung CII).
    Cii,
    /// UN/CEFACT Cross Industry Document of ZUGFeRD 1.0.
    CiiLegacy,
    /// The Italian FatturaPA.
    FatturaPa,
    /// UN/CEFACT supply chain message (Order-X).
    OrderX,
}

/// The standard or format a document says it follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standard {
    /// The German XRechnung (a CIUS of EN 16931).
    XRechnung,
    /// An extension of XRechnung (it adds to the rules and is not a plain XRechnung).
    XRechnungExtension,
    /// Peppol BIS Billing 3.0.
    PeppolBis,
    /// Peppol International Invoicing (PINT).
    PeppolPint,
    /// Factur-X, and ZUGFeRD 2.1 and later, which are the same format.
    FacturX,
    /// ZUGFeRD 2.0.
    Zugferd2,
    /// ZUGFeRD 1.0.
    Zugferd1,
    /// The European standard EN 16931 itself (a CEN syntax binding with no further national rules).
    En16931,
    /// OIOUBL, the Danish UBL profile.
    Oioubl,
    /// The Italian electronic invoice.
    FatturaPa,
    /// Order-X, the order sibling of Factur-X.
    OrderX,
    /// A UBL or CII document that names nothing known (or nothing at all).
    Unspecified,
}

impl Standard {
    pub fn name(self) -> &'static str {
        match self {
            Standard::XRechnung => "XRechnung",
            Standard::XRechnungExtension => "XRechnung extension",
            Standard::PeppolBis => "Peppol BIS Billing 3.0",
            Standard::PeppolPint => "Peppol PINT",
            Standard::FacturX => "Factur-X / ZUGFeRD 2.1+",
            Standard::Zugferd2 => "ZUGFeRD 2.0",
            Standard::Zugferd1 => "ZUGFeRD 1.0",
            Standard::En16931 => "EN 16931",
            Standard::Oioubl => "OIOUBL",
            Standard::FatturaPa => "FatturaPA",
            Standard::OrderX => "Order-X",
            Standard::Unspecified => "no standard named",
        }
    }
}

/// What was recognised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
    pub syntax: Syntax,
    /// `Invoice`, `Credit note`, `Order`, … (the element the document is).
    pub document: String,
    pub standard: Standard,
    /// The profile within the standard: `minimum`, `basic WL`, `basic`, `EN 16931`, `extended` for Factur-X and ZUGFeRD; the
    /// transmission format for FatturaPA (`FPR12`).
    pub profile: Option<String>,
    /// The version of the standard when the document says it (XRechnung `3.0`, FatturaPA `1.2`).
    pub version: Option<String>,
    /// The version of the syntax (UBL `2.1`), when the document says it.
    pub syntax_version: Option<String>,
    /// The identifier the document carries: UBL `CustomizationID`, CII `GuidelineSpecifiedDocumentContextParameter/ID`.
    pub identifier: Option<String>,
    /// UBL `ProfileID` (the Peppol process, say), when there is one.
    pub process: Option<String>,
}

/// The path, among the bundled schemas, of the XML Schema a detected document is written against, when XmlCraft ships it (UBL 2.1
/// Invoice and CreditNote; the UN/CEFACT Cross Industry Invoice D16B). A document is checked against it when it names no schema of its
/// own. FatturaPA has none: its publisher's terms do not allow the schema to be redistributed (AGENTS.md section 1.6).
pub fn bundled_schema(detection: &Detection) -> Option<&'static str> {
    match (detection.syntax, detection.document.as_str()) {
        (Syntax::Ubl, "Invoice") => Some("ubl/maindoc/UBL-Invoice-2.1.xsd"),
        (Syntax::Ubl, "Credit note") => Some("ubl/maindoc/UBL-CreditNote-2.1.xsd"),
        (Syntax::Cii, "Invoice") => Some("cii/CrossIndustryInvoice_100pD16B.xsd"),
        _ => None,
    }
}

fn child<'a>(tree: &'a Dom, el: &El, namespace: &str, local: &str) -> Option<&'a El> {
    el.children.iter().filter_map(|i| tree.elements.get(*i)).find(|c| c.local == local && c.namespace.as_deref() == Some(namespace))
}

fn text_of(el: &El) -> Option<String> {
    let t = el.text.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// Recognise the document `xml`, or none when it is not a known business document.
pub fn detect(xml: &str) -> Option<Detection> {
    let parsed = parse(xml);
    let tree = dom::build(&parsed.syntax());
    let root = tree.elements.first()?;
    let namespace = root.namespace.as_deref().unwrap_or("");
    match (namespace, root.local.as_str()) {
        (UBL_INVOICE, "Invoice") => Some(ubl(&tree, root, "Invoice")),
        (UBL_CREDIT_NOTE, "CreditNote") => Some(ubl(&tree, root, "Credit note")),
        (UBL_ORDER, "Order") => Some(ubl(&tree, root, "Order")),
        (UBL_ORDER_RESPONSE, "OrderResponse") => Some(ubl(&tree, root, "Order response")),
        (UBL_DESPATCH_ADVICE, "DespatchAdvice") => Some(ubl(&tree, root, "Despatch advice")),
        (UBL_APPLICATION_RESPONSE, "ApplicationResponse") => Some(ubl(&tree, root, "Application response")),
        (CII, "CrossIndustryInvoice") => Some(cii(&tree, root)),
        (ZUGFERD_1, "CrossIndustryDocument") => Some(Detection {
            syntax: Syntax::CiiLegacy,
            document: "Invoice".to_owned(),
            standard: Standard::Zugferd1,
            profile: zugferd1_profile(&tree, root),
            version: Some("1.0".to_owned()),
            syntax_version: None,
            identifier: guideline_v1(&tree, root),
            process: None,
        }),
        (ORDER_X, "SCRDMCCBDACIOMessageStructure") => Some(Detection {
            syntax: Syntax::OrderX,
            document: "Order".to_owned(),
            standard: Standard::OrderX,
            profile: None,
            version: None,
            syntax_version: None,
            identifier: None,
            process: None,
        }),
        (ns, "FatturaElettronica") if ns.starts_with(FATTURAPA_PREFIX) || ns.starts_with(FATTURAPA_PREFIX_ALT) => {
            let version = ns.rsplit_once('/').map(|(_, v)| v.trim_start_matches('v').to_owned()).filter(|v| !v.is_empty());
            let format = root.attrs.iter().find(|a| a.local == "versione").map(|a| a.value.clone()).filter(|v| !v.is_empty());
            Some(Detection {
                syntax: Syntax::FatturaPa,
                document: "Invoice".to_owned(),
                standard: Standard::FatturaPa,
                profile: format,
                version,
                syntax_version: None,
                identifier: None,
                process: None,
            })
        }
        _ => None,
    }
}

fn ubl(tree: &Dom, root: &El, document: &str) -> Detection {
    let identifier = child(tree, root, UBL_CBC, "CustomizationID").and_then(text_of);
    let process = child(tree, root, UBL_CBC, "ProfileID").and_then(text_of);
    let syntax_version = child(tree, root, UBL_CBC, "UBLVersionID").and_then(text_of);
    let (standard, version) = identifier.as_deref().map_or((Standard::Unspecified, None), classify_identifier);
    Detection { syntax: Syntax::Ubl, document: document.to_owned(), standard, profile: None, version, syntax_version, identifier, process }
}

fn cii(tree: &Dom, root: &El) -> Detection {
    let context = child(tree, root, CII, "ExchangedDocumentContext");
    let identifier = context
        .and_then(|c| child(tree, c, CII_RAM, "GuidelineSpecifiedDocumentContextParameter"))
        .and_then(|g| child(tree, g, CII_RAM, "ID"))
        .and_then(text_of);
    let (standard, version) = identifier.as_deref().map_or((Standard::Unspecified, None), classify_identifier);
    let profile = identifier.as_deref().and_then(|id| factur_x_profile(id, standard));
    Detection { syntax: Syntax::Cii, document: "Invoice".to_owned(), standard, profile, version, syntax_version: None, identifier, process: None }
}

fn zugferd1_profile(tree: &Dom, root: &El) -> Option<String> {
    let id = guideline_v1(tree, root)?;
    let last = id.rsplit(':').next()?;
    match last {
        "basic" => Some("basic".to_owned()),
        "comfort" => Some("comfort".to_owned()),
        "extended" => Some("extended".to_owned()),
        _ => None,
    }
}

/// ZUGFeRD 1.0: `SpecifiedExchangedDocumentContext/GuidelineSpecifiedDocumentContextParameter/ID` in the 1.0 namespaces.
fn guideline_v1(tree: &Dom, root: &El) -> Option<String> {
    let context = root.children.iter().filter_map(|i| tree.elements.get(*i)).find(|c| c.local == "SpecifiedExchangedDocumentContext")?;
    let guideline =
        context.children.iter().filter_map(|i| tree.elements.get(*i)).find(|c| c.local == "GuidelineSpecifiedDocumentContextParameter")?;
    let id = guideline.children.iter().filter_map(|i| tree.elements.get(*i)).find(|c| c.local == "ID")?;
    text_of(id)
}

/// What a customization or guideline identifier says: the standard, and the version of it where the identifier carries one.
fn classify_identifier(id: &str) -> (Standard, Option<String>) {
    let lower = id.to_ascii_lowercase();
    let version_after = |marker: &str| -> Option<String> {
        let at = lower.find(marker)? + marker.len();
        let version: String = lower.get(at..)?.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let version = version.trim_end_matches('.').to_owned();
        (!version.is_empty()).then_some(version)
    };
    if lower.contains("xrechnung") {
        let extension = lower.contains("extension");
        let version = version_after("xrechnung_").or_else(|| version_after("xrechnung-"));
        return (if extension { Standard::XRechnungExtension } else { Standard::XRechnung }, version);
    }
    if lower.contains("peppol.eu:2017:poacc:billing") {
        return (Standard::PeppolBis, version_after("billing:"));
    }
    if lower.contains("peppol:pint") {
        return (Standard::PeppolPint, None);
    }
    if lower.contains("factur-x.eu") {
        return (Standard::FacturX, None);
    }
    if lower.contains("zugferd.de:2p0") {
        return (Standard::Zugferd2, Some("2.0".to_owned()));
    }
    if lower.starts_with("oioubl") {
        return (Standard::Oioubl, lower.strip_prefix("oioubl-").map(str::to_owned));
    }
    if lower.starts_with("urn:cen.eu:en16931:2017") {
        return (Standard::En16931, None);
    }
    (Standard::Unspecified, None)
}

/// The profile of a Factur-X or ZUGFeRD identifier (the last part of `urn:factur-x.eu:1p0:basicwl`), or `EN 16931` for the plain
/// European identifier of a CII document.
fn factur_x_profile(id: &str, standard: Standard) -> Option<String> {
    let lower = id.to_ascii_lowercase();
    match standard {
        Standard::FacturX | Standard::Zugferd2 => {
            let last = lower.rsplit(':').next().unwrap_or_default();
            Some(
                match last {
                    "minimum" => "minimum",
                    "basicwl" => "basic WL",
                    "basic" => "basic",
                    "comfort" | "en16931" => "EN 16931",
                    "extended" => "extended",
                    _ => return None,
                }
                .to_owned(),
            )
        }
        Standard::En16931 if lower == "urn:cen.eu:en16931:2017" => Some("EN 16931".to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ubl_invoice(customization: &str, extra: &str) -> String {
        format!(
            "<Invoice xmlns=\"{UBL_INVOICE}\" xmlns:cbc=\"{UBL_CBC}\"><cbc:UBLVersionID>2.1</cbc:UBLVersionID><cbc:CustomizationID>{customization}</cbc:CustomizationID>{extra}</Invoice>"
        )
    }

    fn cii_invoice(id: &str) -> String {
        format!(
            "<rsm:CrossIndustryInvoice xmlns:rsm=\"{CII}\" xmlns:ram=\"{CII_RAM}\"><rsm:ExchangedDocumentContext><ram:GuidelineSpecifiedDocumentContextParameter><ram:ID>{id}</ram:ID></ram:GuidelineSpecifiedDocumentContextParameter></rsm:ExchangedDocumentContext></rsm:CrossIndustryInvoice>"
        )
    }

    #[test]
    fn xrechnung_in_ubl_is_recognised_with_its_version_and_an_extension_is_not_a_plain_xrechnung() {
        let d = detect(&ubl_invoice("urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0", "")).expect("recognised");
        assert_eq!(
            (d.syntax, d.standard, d.version.as_deref(), d.syntax_version.as_deref()),
            (Syntax::Ubl, Standard::XRechnung, Some("3.0"), Some("2.1"))
        );
        let old = detect(&ubl_invoice("urn:cen.eu:en16931:2017#compliant#urn:xoev-de:kosit:standard:xrechnung_2.3", "")).expect("recognised");
        assert_eq!((old.standard, old.version.as_deref()), (Standard::XRechnung, Some("2.3")));
        let ext = detect(&ubl_invoice("urn:cen.eu:en16931:2017#conformant#urn:xeinkauf.de:kosit:extension:xrechnung_3.0", "")).expect("recognised");
        assert_eq!(ext.standard, Standard::XRechnungExtension);
    }

    #[test]
    fn peppol_en16931_and_unknown_identifiers_are_told_apart() {
        let p = detect(&ubl_invoice(
            "urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0",
            "<cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID>",
        ))
        .expect("recognised");
        assert_eq!((p.standard, p.version.as_deref()), (Standard::PeppolBis, Some("3.0")));
        assert_eq!(p.process.as_deref(), Some("urn:fdc:peppol.eu:2017:poacc:billing:01:1.0"));
        assert_eq!(detect(&ubl_invoice("urn:cen.eu:en16931:2017", "")).expect("recognised").standard, Standard::En16931);
        assert_eq!(detect(&ubl_invoice("something:else", "")).expect("recognised").standard, Standard::Unspecified);
        assert_eq!(detect(&ubl_invoice("OIOUBL-2.02", "")).expect("recognised").standard, Standard::Oioubl);
        assert_eq!(detect(&ubl_invoice("urn:peppol:pint:billing-1@jp-1", "")).expect("recognised").standard, Standard::PeppolPint);
    }

    #[test]
    fn a_credit_note_and_an_order_say_what_they_are() {
        let note = format!("<CreditNote xmlns=\"{UBL_CREDIT_NOTE}\"/>");
        let d = detect(&note).expect("recognised");
        assert_eq!((d.document.as_str(), d.standard, d.identifier), ("Credit note", Standard::Unspecified, None));
        let order = format!("<Order xmlns=\"{UBL_ORDER}\"/>");
        assert_eq!(detect(&order).expect("recognised").document, "Order");
    }

    #[test]
    fn factur_x_and_zugferd_profiles_are_read_from_the_guideline() {
        for (id, standard, profile) in [
            ("urn:factur-x.eu:1p0:minimum", Standard::FacturX, Some("minimum")),
            ("urn:factur-x.eu:1p0:basicwl", Standard::FacturX, Some("basic WL")),
            ("urn:cen.eu:en16931:2017#compliant#urn:factur-x.eu:1p0:basic", Standard::FacturX, Some("basic")),
            ("urn:cen.eu:en16931:2017#conformant#urn:factur-x.eu:1p0:extended", Standard::FacturX, Some("extended")),
            ("urn:zugferd.de:2p0:minimum", Standard::Zugferd2, Some("minimum")),
            ("urn:cen.eu:en16931:2017", Standard::En16931, Some("EN 16931")),
            ("urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0", Standard::XRechnung, None),
        ] {
            let d = detect(&cii_invoice(id)).expect("recognised");
            assert_eq!((d.syntax, d.standard, d.profile.as_deref()), (Syntax::Cii, standard, profile), "{id}");
            assert_eq!(d.identifier.as_deref(), Some(id));
        }
    }

    #[test]
    fn zugferd_1_fatturapa_and_order_x_are_recognised_by_their_roots() {
        let z = format!(
            "<rsm:CrossIndustryDocument xmlns:rsm=\"{ZUGFERD_1}\"><rsm:SpecifiedExchangedDocumentContext><ram:GuidelineSpecifiedDocumentContextParameter xmlns:ram=\"urn:un:unece:uncefact:data:standard:ReusableAggregateBusinessInformationEntity:12\"><ram:ID>urn:ferd:CrossIndustryDocument:invoice:1p0:comfort</ram:ID></ram:GuidelineSpecifiedDocumentContextParameter></rsm:SpecifiedExchangedDocumentContext></rsm:CrossIndustryDocument>"
        );
        let d = detect(&z).expect("recognised");
        assert_eq!((d.syntax, d.standard, d.profile.as_deref()), (Syntax::CiiLegacy, Standard::Zugferd1, Some("comfort")));
        let f = "<p:FatturaElettronica versione=\"FPR12\" xmlns:p=\"http://ivaservizi.agenziaentrate.gov.it/docs/xsd/fatture/v1.2\"/>";
        let d = detect(f).expect("recognised");
        assert_eq!(
            (d.syntax, d.standard, d.profile.as_deref(), d.version.as_deref()),
            (Syntax::FatturaPa, Standard::FatturaPa, Some("FPR12"), Some("1.2"))
        );
        let o = format!("<rsm:SCRDMCCBDACIOMessageStructure xmlns:rsm=\"{ORDER_X}\"/>");
        assert_eq!(detect(&o).expect("recognised").standard, Standard::OrderX);
    }

    #[test]
    fn other_xml_and_broken_text_are_not_business_documents() {
        for text in ["", "<a/>", "not xml", "<Invoice/>", &format!("<Bill xmlns=\"{UBL_INVOICE}\"/>"), "<Invoice xmlns=\"urn:x\"><a>"] {
            assert_eq!(detect(text), None, "{text:?}");
        }
        // A document that is cut off after the root is still recognised (the editor works on text that is being typed).
        assert!(detect(&format!("<Invoice xmlns=\"{UBL_INVOICE}\"><a>")).is_some());
    }

    proptest::proptest! {
        #[test]
        fn any_text_is_answered_without_a_panic(text in ".{0,300}") {
            let _ = detect(&text);
        }

        #[test]
        fn a_prefix_of_a_real_document_is_answered_without_a_panic(cut in 0usize..400) {
            let doc = ubl_invoice("urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0", "<a>é</a>");
            let at = (0..=cut.min(doc.len())).rev().find(|i| doc.is_char_boundary(*i)).unwrap_or(0);
            let _ = detect(&doc[..at]);
        }
    }

    #[test]
    fn only_the_documents_with_a_bundled_schema_have_one() {
        let path = |text: &str| detect(text).as_ref().and_then(bundled_schema);
        assert_eq!(path(&ubl_invoice("x", "")), Some("ubl/maindoc/UBL-Invoice-2.1.xsd"));
        assert_eq!(path(&format!("<CreditNote xmlns=\"{UBL_CREDIT_NOTE}\"/>")), Some("ubl/maindoc/UBL-CreditNote-2.1.xsd"));
        assert_eq!(path(&format!("<Order xmlns=\"{UBL_ORDER}\"/>")), None);
        assert_eq!(path(&cii_invoice("urn:factur-x.eu:1p0:minimum")), Some("cii/CrossIndustryInvoice_100pD16B.xsd"));
        let fattura = "<p:FatturaElettronica versione=\"FPR12\" xmlns:p=\"http://ivaservizi.agenziaentrate.gov.it/docs/xsd/fatture/v1.2\"/>";
        assert_eq!(path(fattura), None, "the FatturaPA schema is not bundled");
        let zugferd1 = format!("<rsm:CrossIndustryDocument xmlns:rsm=\"{ZUGFERD_1}\"/>");
        assert_eq!(path(&zugferd1), None, "ZUGFeRD 1 is another schema");
    }

    #[test]
    fn an_empty_identifier_names_no_standard() {
        let d = detect(&ubl_invoice("  ", "")).expect("recognised");
        assert_eq!((d.standard, d.identifier), (Standard::Unspecified, None));
    }
}
