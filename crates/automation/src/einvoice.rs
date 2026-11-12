//! `einvoice_detect`: which electronic invoice standard a document follows; `einvoice_validate`: the invoice checked stage by stage
//! against the schemas and rules that ship with the app.
//!
//! The document is an open document (`id`), a file (`path`, through the root confinement) or `text`. Nothing is changed, and nothing is
//! fetched: the validation is offline by construction.

use serde_json::{Value, json};
use xmlcraft_einvoice::{Detection, Standard, Syntax, detect};

use xmlcraft_einvoice::validate::{MAX_FINDINGS, Report, Stage, Status};
use xmlcraft_text::Buffer;

use crate::documents::{fail, json_result, language_source_properties, source, source_properties};

use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
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
        },
        ToolDef {
            name: "einvoice_validate",
            title: "Validate an electronic invoice",
            description: "Validate an electronic invoice stage by stage against the schemas and rules that ship with XmlCraft, offline: `well-formedness`, \
                      `detection` (syntax and the specification it claims), `schema` (the UBL 2.1 or UN/CEFACT CII D16B XML Schema), `en16931` (the CEN \
                      EN 16931 business rules for the syntax, Schematron 1.3.17) and `cius` (the XRechnung 3.0 rules when the document claims XRechnung; \
                      the Peppol BIS Billing 3 rules are not bundled, FatturaPA has an XML Schema only, which is not bundled either). A stage runs when \
                      the earlier ones did not fail fatally (schema problems do not stop the rules). Each finding has its `stage`, `rule` id (BR-CO-15, \
                      BR-DE-1), `severity` (fatal, warning, information, as the rule says), `message`, place (`start`, `end`, `line`, `column` of the \
                      node), the node's `path` and the rule's `test`. `totals` count the checks: passed, failed, not fired, and `not_evaluated` (a \
                      check the engine could not evaluate, listed with its reason; it is never counted as passed or failed).",
            input_schema: json!({
                "type": "object",
                "properties": validate_properties(),
                "additionalProperties": false
            }),
            read_only: true,
            destructive: false,
            handler: einvoice_validate,
        },
        ToolDef {
            name: "einvoice_model",
            title: "Read an electronic invoice into a neutral model",
            description: "Read a UBL Invoice or CreditNote or a UN/CEFACT Cross Industry Invoice into one model that does not depend on the syntax: \
                      `number`, `type_code`, `issue_date`, `due_date`, `currency`, `buyer_reference`, `order_reference`, `notes`, `seller` and `buyer` (and \
                      `payee`: `name`, `trading_name`, `id`, `address` with `lines`, `city`, `postcode`, `subdivision`, `country`, `vat_id`, \
                      `tax_registration`, `legal_id`, `contact` with `name`, `phone`, `email`, `electronic_address`), `delivery` (`date`, `party_name`, \
                      `address`), `payment` (`means_code`, `reference`, `iban`, `bic`, `account_name`, `mandate`), `payment_terms`, \
                      `allowances_charges`, `vat_breakdown` (`category`, `rate`, `taxable`, `tax`, `exemption_reason`), `totals` (`line_total`, \
                      `allowance_total`, `charge_total`, `tax_exclusive`, `tax_total`, `tax_inclusive`, `prepaid`, `rounding`, `payable`) and `lines` \
                      (`id`, `name`, `description`, `note`, `quantity`, `unit`, `net_price`, `line_total`, `vat_category`, `vat_rate`, \
                      `seller_item_id`, `period_start`, `period_end`). A value is left out when the document lacks it. Each value has the `text` the \
                      document has, its place (`start`, `end`, `line`, `column`), and, where it applies, the exact `decimal` (as a string, never a \
                      float), the ISO `date`, and the `unit` (currency of an amount, unit code of a quantity). At most 100,000 lines are read \
                      (`lines_total` counts them all); `cut` says what was left out. `recognised` is false for any other document.",
            input_schema: json!({ "type": "object", "properties": source_properties(), "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: einvoice_model,
        },
        ToolDef {
            name: "einvoice_render_pdf",
            title: "Make a PDF of an electronic invoice (ZUGFeRD / Factur-X hybrid)",
            description: "Make a PDF that shows an electronic invoice to people and carries its XML unchanged (byte for byte), and write it to `out` \
                      (inside the root; an existing file is replaced only with `overwrite: true`). The invoice is an open document (`id`), a file \
                      (`path`, whose bytes are embedded as they are) or `text`. `template` is `de` (the built-in German layout, address in the \
                      DIN 5008 window), `en`, or the path of a template file (the `invoice-template` format: header, body and footer of blocks, \
                      columns, fields, tables and totals with `{path|filter}` placeholders). A UN/CEFACT CII invoice with a Factur-X profile \
                      is embedded as `factur-x.xml` (`xrechnung.xml` for XRechnung) with the Factur-X XMP metadata and the `/AFRelationship` the \
                      specification gives; a UBL invoice is attached as `invoice.xml` (Factur-X is CII only). PDF/A-3 needs an output intent with \
                      an ICC profile, which XmlCraft does not ship: give `icc_profile` (the path of an RGB, gray or CMYK output or display profile) \
                      for the file to say it is PDF/A-3b. Returns `pages`, `bytes`, `warnings` (placeholders that name nothing, characters the \
                      font lacks), `facturx` (`hybrid`, `profile`, `file_name`, `relationship`, `reason`) and `pdfa` (with `pdfa_reason`). \
                      Neither PDF/A nor Factur-X conformance is verified: check the file with a validator before relying on it.",
            input_schema: json!({ "type": "object", "properties": render_properties(), "required": ["out"], "additionalProperties": false }),
            read_only: false,
            destructive: false,
            handler: einvoice_render_pdf,
        },
    ]
}

fn validate_properties() -> Value {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert(
            "stages".into(),
            json!({ "type": "array", "items": { "type": "string", "enum": ["schema", "en16931", "cius"] }, "description": "The stages to run besides well-formedness and detection (all when left out)" }),
        );
        map.insert(
            "max_findings".into(),
            json!({ "type": "integer", "minimum": 0, "maximum": MAX_FINDINGS, "description": "The most findings listed (default and most 1000; all are counted)" }),
        );
    }
    properties
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

/// The most checks that were not evaluated the tool lists (all are counted).
const MAX_NOT_EVALUATED_LISTED: usize = 200;

fn status_json(status: &Status) -> Value {
    match status {
        Status::Passed => json!({ "status": "passed" }),
        Status::Failed => json!({ "status": "failed" }),
        Status::NotRun(why) => json!({ "status": "not-run", "reason": why }),
        Status::Error(why) => json!({ "status": "error", "reason": why }),
    }
}

fn totals_json(t: &xmlcraft_schematron::Totals) -> Value {
    json!({ "checks": t.checks, "passed": t.passed, "failed": t.failed, "not_fired": t.not_fired, "not_evaluated": t.not_evaluated })
}

fn report_json(report: &Report, text: &str) -> Value {
    let buffer = Buffer::from_text(text);
    let stages: Vec<Value> = report
        .stages
        .iter()
        .map(|s| {
            let mut v = status_json(&s.status);
            v["stage"] = json!(s.stage.key());
            v["against"] = json!(s.against);
            v["findings"] = json!(s.findings);
            if let Some(t) = &s.totals {
                v["totals"] = totals_json(t);
            }
            v
        })
        .collect();
    let findings: Vec<Value> = report
        .findings
        .iter()
        .map(|f| {
            let (end_line, end_column) = crate::documents::position_json(&buffer, f.span.end);
            json!({
                "stage": f.stage.key(),
                "rule": f.rule,
                "severity": f.severity.key(),
                "message": f.message,
                "start": f.span.start,
                "end": f.span.end,
                "line": f.line,
                "column": f.column,
                "end_line": end_line,
                "end_column": end_column,
                "path": f.path,
                "test": f.test,
                "context": f.context,
            })
        })
        .collect();
    let not_evaluated: Vec<Value> = report
        .not_evaluated
        .iter()
        .take(MAX_NOT_EVALUATED_LISTED)
        .map(|n| json!({ "stage": n.stage.key(), "rule": n.rule, "test": n.test, "context": n.context, "reason": n.reason }))
        .collect();
    let detection = match &report.detection {
        Some(d) => describe(d),
        None => json!({ "recognised": false }),
    };
    json!({
        "valid": report.valid(),
        "detection": detection,
        "stages": stages,
        "findings": findings,
        "findings_total": report.findings_total,
        "truncated": report.findings_total > report.findings.len(),
        "totals": totals_json(&report.totals),
        "not_evaluated": not_evaluated,
        "not_evaluated_total": report.not_evaluated_total,
    })
}

fn einvoice_validate(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let stages = match args.get("stages") {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) => {
            let mut stages = Vec::new();
            for item in items {
                let stage = item
                    .as_str()
                    .and_then(Stage::from_key)
                    .filter(|s| matches!(s, Stage::Schema | Stage::En16931 | Stage::Cius))
                    .ok_or_else(|| ToolError::InvalidArgs(format!("stages: {item} is not one of schema, en16931, cius")))?;
                stages.push(stage);
            }
            Some(stages)
        }
        Some(_) => return Err(ToolError::InvalidArgs("stages is a list of schema, en16931, cius".into())),
    };
    let max_findings = match args.get("max_findings") {
        None | Some(Value::Null) => MAX_FINDINGS,
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n <= MAX_FINDINGS)
            .ok_or_else(|| ToolError::InvalidArgs(format!("max_findings is a whole number from 0 to {MAX_FINDINGS}")))?,
    };
    let s = source(a, args)?;
    if !s.language.is_xml() {
        return Err(ToolError::InvalidArgs("einvoice_validate reads XML documents".into()));
    }
    let location = s.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let report = xmlcraft_engine::einvoice::einvoice_validate(&s.text, &location, stages, max_findings)
        .map_err(|why| ToolError::Failed(format!("the validation failed: {why}")))?;
    let mut value = report_json(&report, &s.text);
    value["lossy"] = json!(s.lossy);
    json_result(value)
}

/// A decimal as a plain string with the document's digits (`-12.50`).
fn decimal_string(d: xmlcraft_einvoice::model::Decimal) -> String {
    let (whole, fraction) = d.digits();
    let sign = if d.is_negative() { "-" } else { "" };
    if fraction.is_empty() { format!("{sign}{whole}") } else { format!("{sign}{whole}.{fraction}") }
}

fn field_json(f: &xmlcraft_einvoice::model::Field, buffer: &Buffer) -> Value {
    let (line, column) = crate::documents::position_json(buffer, f.range.start);
    let mut v = json!({ "text": f.text, "start": f.range.start, "end": f.range.end, "line": line, "column": column });
    if let Some(d) = f.decimal {
        v["decimal"] = json!(decimal_string(d));
    }
    if let Some(d) = f.date {
        v["date"] = json!(d.format("%Y-%m-%d").to_string());
    }
    if let Some(u) = &f.unit {
        v["unit"] = json!(u);
    }
    v
}

fn node_json(node: &xmlcraft_einvoice::model::Node<'_>, buffer: &Buffer) -> Value {
    use xmlcraft_einvoice::model::Node;
    match node {
        Node::Field(f) => field_json(f, buffer),
        Node::List(items) => Value::Array(items.iter().map(|n| node_json(n, buffer)).collect()),
        Node::Group(children) => Value::Object(children.iter().map(|(name, n)| ((*name).to_owned(), node_json(n, buffer))).collect()),
    }
}

fn einvoice_model(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let s = source(a, args)?;
    if !s.language.is_xml() {
        return Err(ToolError::InvalidArgs("einvoice_model reads XML documents".into()));
    }
    let model = xmlcraft_engine::einvoice::einvoice_model(&s.text).map_err(|why| ToolError::Failed(format!("reading the invoice failed: {why}")))?;
    let Some(invoice) = model else {
        return json_result(
            json!({ "recognised": false, "reason": "the document is not a UBL Invoice or CreditNote or a CII CrossIndustryInvoice" }),
        );
    };
    let buffer = Buffer::from_text(&s.text);
    let mut value = node_json(&invoice.tree(), &buffer);
    value["recognised"] = json!(true);
    value["syntax"] = json!(syntax_name(invoice.syntax));
    value["document"] = json!(invoice.document);
    value["lines_total"] = json!(invoice.lines_total);
    value["cut"] = json!(invoice.cut);
    value["lossy"] = json!(s.lossy);
    json_result(value)
}

fn render_properties() -> Value {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert(
            "template".into(),
            json!({ "type": "string", "description": "`de` (default), `en`, or the path of an invoice template file inside the root" }),
        );
        map.insert("out".into(), json!({ "type": "string", "description": "The PDF file to write, inside the root" }));
        map.insert("overwrite".into(), json!({ "type": "boolean", "description": "Replace `out` when it exists (default false)" }));
        map.insert(
            "icc_profile".into(),
            json!({ "type": "string", "description": "The path of an ICC output or display profile for the PDF/A-3 output intent" }),
        );
    }
    properties
}

/// The largest template or ICC profile read.
const MAX_SIDE_FILE: usize = 4 * 1024 * 1024;

fn read_side_file(a: &Automation, path: &str, what: &str) -> Result<Vec<u8>> {
    let resolved = a.confinement.resolve_read(path)?;
    let loaded = xmlcraft_platform::files::read_file(&resolved).map_err(|e| ToolError::Failed(format!("{what} {}: {e}", resolved.display())))?;
    if loaded.bytes.len() > MAX_SIDE_FILE {
        return Err(ToolError::Failed(format!("{what} {} is larger than {} MiB", resolved.display(), MAX_SIDE_FILE / (1024 * 1024))));
    }
    Ok(loaded.bytes)
}

fn einvoice_render_pdf(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let out = args.get("out").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("`out` (the PDF to write) is required".into()))?;
    let overwrite = match args.get("overwrite") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(ToolError::InvalidArgs("overwrite is true or false".into())),
    };
    let template_text = match args.get("template") {
        None | Some(Value::Null) => None,
        Some(Value::String(t)) if t == "de" || t == "en" => None,
        Some(Value::String(path)) => {
            let bytes = read_side_file(a, path, "the template")?;
            Some(String::from_utf8(bytes).map_err(|_| ToolError::Failed(format!("the template {path} is not UTF-8")))?)
        }
        Some(_) => return Err(ToolError::InvalidArgs("template is `de`, `en` or the path of a template file".into())),
    };
    let template = match (args.get("template").and_then(Value::as_str), template_text.as_deref()) {
        (_, Some(t)) => xmlcraft_invoice_pdf::TemplateSource::Text(t),
        (Some("en"), None) => xmlcraft_invoice_pdf::TemplateSource::En,
        _ => xmlcraft_invoice_pdf::TemplateSource::De,
    };
    let icc = match args.get("icc_profile") {
        None | Some(Value::Null) => None,
        Some(Value::String(path)) => Some(read_side_file(a, path, "the ICC profile")?),
        Some(_) => return Err(ToolError::InvalidArgs("icc_profile is the path of an ICC profile".into())),
    };
    let target = a.confinement.resolve_write(out)?;
    if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
        return fail(format!("{} is a symbolic link, and is not written through", target.display()));
    }
    if target.exists() && !overwrite {
        return fail(format!("{} exists: pass overwrite: true to replace it", target.display()));
    }
    let s = source(a, args)?;
    if !s.language.is_xml() {
        return Err(ToolError::InvalidArgs("einvoice_render_pdf reads XML documents".into()));
    }
    if s.lossy {
        return fail("the file has bytes that are not valid in its encoding; it would not be embedded unchanged, so it is not rendered");
    }
    // The bytes embedded: a file's own bytes (whatever its encoding); otherwise the text, which is UTF-8.
    let file_bytes = match args.get("path").and_then(Value::as_str) {
        Some(path) => Some(read_side_file_any(a, path)?).filter(|b| !b.starts_with(b"%PDF")),
        None => None,
    };
    let bytes = match file_bytes {
        Some(b) => b,
        None => {
            if let Some(encoding) = crate::pdf::declared_encoding(&s.text)
                && !encoding.eq_ignore_ascii_case("utf-8")
                && !encoding.eq_ignore_ascii_case("utf8")
            {
                return fail(format!(
                    "the document declares the encoding {encoding}, and its text would be embedded as UTF-8: give the file by `path` to embed its bytes"
                ));
            }
            s.text.as_bytes().to_vec()
        }
    };
    let time = xmlcraft_resolve::Clock::now(&xmlcraft_platform::network::SystemClock);
    let options = xmlcraft_invoice_pdf::Options { template, time, icc_profile: icc.as_deref() };
    let rendered = xmlcraft_engine::einvoice::einvoice_render_pdf(&s.text, &bytes, &options)
        .map_err(|why| ToolError::Failed(format!("rendering the invoice failed: {why}")))?
        .map_err(|e| match e {
            xmlcraft_invoice_pdf::RenderError::NotAnInvoice(why) => ToolError::InvalidArgs(why),
            other => ToolError::Failed(other.to_string()),
        })?;
    xmlcraft_platform::files::write_file(&target, &rendered.bytes).map_err(|e| ToolError::Failed(format!("{}: {e}", target.display())))?;
    let f = &rendered.facturx;
    json_result(json!({
        "written": target.display().to_string(),
        "pages": rendered.pages,
        "bytes": rendered.bytes.len(),
        "warnings": rendered.warnings,
        "facturx": { "hybrid": f.hybrid, "profile": f.profile, "file_name": f.file_name, "relationship": f.relationship, "reason": f.reason },
        "pdfa": rendered.pdfa,
        "pdfa_reason": rendered.pdfa_reason,
        "verified": false,
    }))
}

/// The bytes of the invoice file given by `path` (bounded like the documents the tools read).
fn read_side_file_any(a: &Automation, path: &str) -> Result<Vec<u8>> {
    let resolved = a.confinement.resolve_read(path)?;
    xmlcraft_platform::files::read_file(&resolved).map(|l| l.bytes).map_err(|e| ToolError::Failed(format!("{}: {e}", resolved.display())))
}
