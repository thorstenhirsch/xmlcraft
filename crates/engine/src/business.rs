//! What a business document is, for the chip in the header: an e-invoice (XRechnung, ZUGFeRD / Factur-X, Peppol BIS, FatturaPA, plain
//! UBL or CII…) or an XBRL instance, recognised from its text (`xmlcraft_einvoice::detect`, `xmlcraft_xbrl`). Cheap to refuse: a text
//! that names none of their namespaces is not read.

use xmlcraft_einvoice::{Standard, Syntax, detect};
use xmlcraft_xbrl::{Level, check, is_xbrl, read};

/// Texts longer than this are not looked at.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// What the header says of a business document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusinessChip {
    /// A few words: `XRechnung 3.0 · Invoice`, `XBRL · 12 facts`.
    pub label: String,
    /// The details, one to a line.
    pub tip: String,
    /// Something in it is wrong (XBRL findings that are errors).
    pub warn: bool,
}

fn syntax_name(syntax: Syntax) -> &'static str {
    match syntax {
        Syntax::Ubl => "UBL",
        Syntax::Cii => "UN/CEFACT CII",
        Syntax::CiiLegacy => "UN/CEFACT CII (ZUGFeRD 1.0)",
        Syntax::FatturaPa => "FatturaPA",
        Syntax::OrderX => "Order-X",
    }
}

/// What `text` is, if it is an e-invoice or an XBRL instance.
pub fn chip_for(text: &str) -> Option<BusinessChip> {
    if text.len() > MAX_BYTES {
        return None;
    }
    if let Some(d) = detect(text) {
        let standard = if d.standard == Standard::Unspecified { syntax_name(d.syntax).to_owned() } else { d.standard.name().to_owned() };
        let version = d.version.as_deref().or(d.syntax_version.as_deref()).map(|v| format!(" {v}")).unwrap_or_default();
        let detail = d.profile.as_deref().map_or(d.document.clone(), |p| format!("{} ({p})", d.document));
        let mut tip = vec![format!("{standard}{version}: {}", d.document), format!("Syntax: {}", syntax_name(d.syntax))];
        if let Some(p) = &d.profile {
            tip.push(format!("Profile: {p}"));
        }
        if let Some(i) = &d.identifier {
            tip.push(format!("Identifier: {i}"));
        }
        if let Some(p) = &d.process {
            tip.push(format!("Process: {p}"));
        }
        return Some(BusinessChip { label: format!("{standard}{version} · {detail}"), tip: tip.join("\n"), warn: false });
    }
    if is_xbrl(text)
        && let Ok(instance) = read(text)
    {
        let findings = check(&instance);
        let errors = findings.iter().filter(|f| f.level == Level::Error).count();
        let facts = instance.facts.len();
        let mut label = format!("XBRL · {facts} {}", if facts == 1 { "fact" } else { "facts" });
        if errors > 0 {
            label.push_str(&format!(" · {errors} {}", if errors == 1 { "error" } else { "errors" }));
        }
        let tip = format!(
            "{}\n{} contexts, {} units, {facts} facts\n{} schema reference(s)\n{} finding(s) of the instance itself (taxonomy rules are not checked yet)",
            match instance.kind {
                xmlcraft_xbrl::Kind::Instance => "XBRL 2.1 instance",
                xmlcraft_xbrl::Kind::Inline => "Inline XBRL",
            },
            instance.contexts.len(),
            instance.units.len(),
            instance.schema_refs.len(),
            findings.len()
        );
        return Some(BusinessChip { label, tip, warn: errors > 0 });
    }
    None
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const XRECHNUNG: &str = r#"<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2" xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2"><cbc:UBLVersionID>2.1</cbc:UBLVersionID><cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0</cbc:CustomizationID><cbc:ID>1</cbc:ID></Invoice>"#;

    const XBRL: &str = r#"<xbrli:xbrl xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:iso4217="http://www.xbrl.org/2003/iso4217" xmlns:eg="http://example.com/eg"><link:schemaRef xlink:type="simple" xlink:href="eg.xsd"/><xbrli:context id="c"><xbrli:entity><xbrli:identifier scheme="http://example.com">x</xbrli:identifier></xbrli:entity><xbrli:period><xbrli:instant>2025-12-31</xbrli:instant></xbrli:period></xbrli:context><xbrli:unit id="u"><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unit><eg:Revenue contextRef="c" unitRef="u" decimals="0">10</eg:Revenue></xbrli:xbrl>"#;

    #[test]
    fn an_e_invoice_is_named_by_its_standard_version_and_document() {
        let c = chip_for(XRECHNUNG).expect("an invoice");
        assert!(c.label.starts_with("XRechnung") && c.label.contains("Invoice"), "{}", c.label);
        assert!(
            !c.warn && c.tip.contains("UBL") && c.tip.contains("Identifier: urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0"),
            "{}",
            c.tip
        );
        let plain = chip_for(r#"<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2"/>"#).expect("a UBL invoice");
        assert!(plain.label.starts_with("UBL") && plain.label.contains("Invoice"), "{}", plain.label);
    }

    #[test]
    fn an_xbrl_instance_says_how_many_facts_it_has_and_warns_of_errors() {
        let c = chip_for(XBRL).expect("an instance");
        assert_eq!((c.label.as_str(), c.warn), ("XBRL · 1 fact", false));
        assert!(c.tip.contains("1 contexts, 1 units, 1 facts"), "{}", c.tip);
        let broken = XBRL.replace("contextRef=\"c\"", "contextRef=\"missing\"");
        let c = chip_for(&broken).expect("an instance");
        assert!(c.warn && c.label.contains("error"), "{}", c.label);
    }

    #[test]
    fn other_documents_and_texts_that_are_too_large_or_broken_have_no_chip() {
        for text in ["<a/>", "", "not xml <", "<html><body/></html>", "<xbrli:xbrl"] {
            assert_eq!(chip_for(text), None, "{text:?}");
        }
        assert_eq!(chip_for(&format!("{XRECHNUNG}{}", " ".repeat(MAX_BYTES))), None);
    }
}
