//! `einvoice_detect`: which electronic invoice standard a document follows.
//!
//! The document is an open document (`id`), a file (`path`, through the root confinement) or `text`. Nothing is changed.

use serde_json::{Value, json};
use xmlcraft_einvoice::{Detection, Standard, Syntax, detect};

use crate::documents::{json_result, language_source_properties, source};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result};

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "einvoice_detect",
        title: "Recognise an electronic invoice",
        description: "Say whether a document is an electronic invoice (or a similar business document) and which standard it follows: the `syntax` (ubl, cii, \
                      cii-legacy for ZUGFeRD 1.0, fatturapa, order-x), the kind of `document` (Invoice, Credit note, Order, …), the `standard` (XRechnung, \
                      XRechnung extension, Peppol BIS Billing 3.0, Peppol PINT, Factur-X / ZUGFeRD 2.1+, ZUGFeRD 2.0, ZUGFeRD 1.0, EN 16931, OIOUBL, FatturaPA, \
                      Order-X, or `no standard named`), its `profile` (minimum, basic WL, basic, EN 16931, extended), `version`, and the `identifier` the document \
                      carries (UBL CustomizationID, CII guideline ID). It reads what the document says; it does not validate it. `recognised` is false for any \
                      other XML.",
        input_schema: json!({ "type": "object", "properties": language_source_properties(), "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: einvoice_detect,
    }]
}

fn syntax_name(syntax: Syntax) -> &'static str {
    match syntax {
        Syntax::Ubl => "ubl",
        Syntax::Cii => "cii",
        Syntax::CiiLegacy => "cii-legacy",
        Syntax::FatturaPa => "fatturapa",
        Syntax::OrderX => "order-x",
    }
}

fn describe(d: &Detection) -> Value {
    json!({
        "recognised": true,
        "syntax": syntax_name(d.syntax),
        "document": d.document,
        "standard": d.standard.name(),
        "named": d.standard != Standard::Unspecified,
        "profile": d.profile,
        "version": d.version,
        "syntax_version": d.syntax_version,
        "identifier": d.identifier,
        "process": d.process,
    })
}

fn einvoice_detect(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let s = source(a, args)?;
    if !s.language.is_xml() {
        return json_result(json!({ "recognised": false, "reason": "the document is not XML" }));
    }
    match detect(&s.text) {
        Some(d) => json_result(describe(&d)),
        None => json_result(json!({ "recognised": false, "reason": "the document is not a known electronic invoice or business document" })),
    }
}
