//! `cargo xtask pdf-oracle` and `cargo xtask einvoice-oracle`: XmlCraft's electronic-invoice output checked by external validators,
//! run as black-box processes when they are installed locally (`AGENTS.md` §2). CI has none of them, so each command says it is
//! skipped and succeeds when its tools are absent.
//!
//! - `pdf-oracle` renders a fixed set of synthetic invoices (`crates/invoice-pdf/tests/fixtures`, `crates/einvoice/tests/fixtures`,
//!   the Factur-X profiles MINIMUM, BASIC WL, BASIC, EN 16931, EXTENDED and XRECHNUNG, a UBL invoice, a 300-line invoice and one with
//!   characters the font lacks) with the templates `de` and `en`, then checks every PDF with veraPDF (`VERAPDF`: the greenfield CLI
//!   jar, `bin/cli-<version>.jar` of an installation), as PDF/A-3b and with its flavour auto-detection, and with Mustang (`MUSTANG_JAR`:
//!   `Mustang-CLI-<version>.jar`, action `validate`: veraPDF plus the Factur-X / ZUGFeRD XMP and XML checks).
//! - `einvoice-oracle` runs the EN 16931 and XRechnung rules, compiled to XSLT by their publishers (`EINVOICE_XSLT_DIR`: the unpacked
//!   release archives of the versions XmlCraft bundles), with Saxon-HE (`SAXON_JAR`: its jar, or a class path with the xmlresolver jars
//!   it needs) over the fixtures and systematically broken variants of them, and compares the failed assertions and successful
//!   reports (rule id and location) with what `einvoice_validate` reports for the same documents.
//!
//! `JAVA` names the Java launcher (default `java`). Nothing is downloaded here; how to get the tools is in `docs/target-app-parity.md`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use xmlcraft_einvoice::validate::{MAX_FINDINGS, Stage, Status};

use crate::gates::root;

/// 2026-11-12 00:00:00 UTC: the creation time of every rendered file, so that runs are repeatable.
const TIME: i64 = 1_794_441_600;

/// The guideline identifier of the EN 16931 fixture, which the profile variants replace.
const EN16931_ID: &str = "<ram:ID>urn:cen.eu:en16931:2017</ram:ID>";

struct Invoice {
    name: String,
    text: String,
}

fn read(rel: &str) -> Result<String> {
    let path = root().join(rel);
    fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

/// `text` with the first `from` replaced by `to`; an error when `from` is not there (a fixture changed under the oracle).
fn replace_first(text: &str, from: &str, to: &str) -> Result<String> {
    if !text.contains(from) {
        bail!("the fixture no longer contains {from:?}");
    }
    Ok(text.replacen(from, to, 1))
}

/// `text` without the first element that starts with `open` (through its end tag, with the line's indentation).
fn remove_element(text: &str, open: &str) -> Result<String> {
    let start = text.find(open).with_context(|| format!("the fixture no longer contains {open:?}"))?;
    let name: String = open.trim_start_matches('<').chars().take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/').collect();
    let close = format!("</{name}>");
    let end = if open.ends_with("/>") {
        start.saturating_add(open.len())
    } else {
        let after = text.get(start..).unwrap_or_default();
        start.saturating_add(after.find(&close).with_context(|| format!("no {close} after {open:?}"))?).saturating_add(close.len())
    };
    let line_start = text.get(..start).and_then(|s| s.rfind('\n')).map_or(start, |i| i.saturating_add(1));
    let from = if text.get(line_start..start).is_some_and(|s| s.trim().is_empty()) { line_start.saturating_sub(1) } else { start };
    Ok(format!("{}{}", text.get(..from).unwrap_or_default(), text.get(end..).unwrap_or_default()))
}

fn cents(c: u64) -> String {
    format!("{}.{:02}", c / 100, c % 100)
}

/// The EN 16931 fixture with `n` lines of 10.00 at 19 % and totals to match.
fn long_invoice(base: &str, n: u64) -> Result<String> {
    let first = base.find("    <ram:IncludedSupplyChainTradeLineItem>").context("no line in the EN 16931 fixture")?;
    let agreement = base.find("    <ram:ApplicableHeaderTradeAgreement>").context("no agreement in the EN 16931 fixture")?;
    let settlement = base.find("    <ram:ApplicableHeaderTradeSettlement>").context("no settlement in the EN 16931 fixture")?;
    let mut out = String::from(base.get(..first).unwrap_or_default());
    for i in 1..=n {
        out.push_str(&format!(
            "    <ram:IncludedSupplyChainTradeLineItem>\n      <ram:AssociatedDocumentLineDocument>\n        <ram:LineID>{i}</ram:LineID>\n      </ram:AssociatedDocumentLineDocument>\n      <ram:SpecifiedTradeProduct>\n        <ram:Name>Artikel {i} mit einer etwas längeren Bezeichnung</ram:Name>\n      </ram:SpecifiedTradeProduct>\n      <ram:SpecifiedLineTradeAgreement>\n        <ram:NetPriceProductTradePrice>\n          <ram:ChargeAmount>10.00</ram:ChargeAmount>\n        </ram:NetPriceProductTradePrice>\n      </ram:SpecifiedLineTradeAgreement>\n      <ram:SpecifiedLineTradeDelivery>\n        <ram:BilledQuantity unitCode=\"C62\">1</ram:BilledQuantity>\n      </ram:SpecifiedLineTradeDelivery>\n      <ram:SpecifiedLineTradeSettlement>\n        <ram:ApplicableTradeTax>\n          <ram:TypeCode>VAT</ram:TypeCode>\n          <ram:CategoryCode>S</ram:CategoryCode>\n          <ram:RateApplicablePercent>19</ram:RateApplicablePercent>\n        </ram:ApplicableTradeTax>\n        <ram:SpecifiedTradeSettlementLineMonetarySummation>\n          <ram:LineTotalAmount>10.00</ram:LineTotalAmount>\n        </ram:SpecifiedTradeSettlementLineMonetarySummation>\n      </ram:SpecifiedLineTradeSettlement>\n    </ram:IncludedSupplyChainTradeLineItem>\n"
        ));
    }
    out.push_str(base.get(agreement..settlement).unwrap_or_default());
    let net = n.saturating_mul(1000);
    let tax = n.saturating_mul(190);
    let gross = net.saturating_add(tax);
    let (net, tax, gross) = (cents(net), cents(tax), cents(gross));
    out.push_str(&format!(
        "    <ram:ApplicableHeaderTradeSettlement>\n      <ram:InvoiceCurrencyCode>EUR</ram:InvoiceCurrencyCode>\n      <ram:SpecifiedTradeSettlementPaymentMeans>\n        <ram:TypeCode>58</ram:TypeCode>\n        <ram:PayeePartyCreditorFinancialAccount>\n          <ram:IBANID>DE02120300000000202051</ram:IBANID>\n        </ram:PayeePartyCreditorFinancialAccount>\n      </ram:SpecifiedTradeSettlementPaymentMeans>\n      <ram:ApplicableTradeTax>\n        <ram:CalculatedAmount>{tax}</ram:CalculatedAmount>\n        <ram:TypeCode>VAT</ram:TypeCode>\n        <ram:BasisAmount>{net}</ram:BasisAmount>\n        <ram:CategoryCode>S</ram:CategoryCode>\n        <ram:RateApplicablePercent>19</ram:RateApplicablePercent>\n      </ram:ApplicableTradeTax>\n      <ram:SpecifiedTradePaymentTerms>\n        <ram:DueDateDateTime>\n          <udt:DateTimeString format=\"102\">20261201</udt:DateTimeString>\n        </ram:DueDateDateTime>\n      </ram:SpecifiedTradePaymentTerms>\n      <ram:SpecifiedTradeSettlementHeaderMonetarySummation>\n        <ram:LineTotalAmount>{net}</ram:LineTotalAmount>\n        <ram:TaxBasisTotalAmount>{net}</ram:TaxBasisTotalAmount>\n        <ram:TaxTotalAmount currencyID=\"EUR\">{tax}</ram:TaxTotalAmount>\n        <ram:GrandTotalAmount>{gross}</ram:GrandTotalAmount>\n        <ram:DuePayableAmount>{gross}</ram:DuePayableAmount>\n      </ram:SpecifiedTradeSettlementHeaderMonetarySummation>\n    </ram:ApplicableHeaderTradeSettlement>\n  </rsm:SupplyChainTradeTransaction>\n</rsm:CrossIndustryInvoice>\n"
    ));
    Ok(out)
}

/// The EN 16931 fixture under another guideline identifier.
fn with_guideline(base: &str, id: &str) -> Result<String> {
    replace_first(base, EN16931_ID, &format!("<ram:ID>{id}</ram:ID>"))
}

const XRECHNUNG_ID: &str = "urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0";

/// The well-formed invoices both oracles start from: every Factur-X profile, XRechnung in CII and UBL, plain EN 16931 in both.
fn invoices() -> Result<Vec<Invoice>> {
    let en16931 = read("crates/invoice-pdf/tests/fixtures/en16931-cii.xml")?;
    let ubl = read("crates/einvoice/tests/fixtures/xrechnung-ubl.xml")?;
    let inv = |name: &str, text: String| Invoice { name: name.to_owned(), text };
    Ok(vec![
        inv("minimum", read("crates/invoice-pdf/tests/fixtures/minimum-cii.xml")?),
        inv("basic-wl", read("crates/invoice-pdf/tests/fixtures/basicwl-cii.xml")?),
        // Factur-X BASIC has no seller contact (BG-6).
        inv(
            "basic",
            remove_element(&with_guideline(&en16931, "urn:cen.eu:en16931:2017#compliant#urn:factur-x.eu:1p0:basic")?, "<ram:DefinedTradeContact>")?,
        ),
        inv("en16931", en16931.clone()),
        inv("extended", with_guideline(&en16931, "urn:cen.eu:en16931:2017#conformant#urn:factur-x.eu:1p0:extended")?),
        inv("xrechnung", with_guideline(&en16931, XRECHNUNG_ID)?),
        inv("xrechnung-small", read("crates/einvoice/tests/fixtures/xrechnung-cii.xml")?),
        inv(
            "ubl-en16931",
            replace_first(
                &ubl,
                "<cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0</cbc:CustomizationID>",
                "<cbc:CustomizationID>urn:cen.eu:en16931:2017</cbc:CustomizationID>",
            )?,
        ),
        inv("ubl-xrechnung", ubl),
    ])
}

// --- small readers of the validators' XML reports (machine-written, so a scan of their tags is enough) ---------------------------

struct Tag {
    /// The local name (after any prefix).
    name: String,
    attrs: BTreeMap<String, String>,
    /// Where the tag ends in the text (the content starts there).
    end: usize,
    closing: bool,
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(rest.get(..amp).unwrap_or_default());
        let after = rest.get(amp..).unwrap_or_default();
        let Some(semi) = after.find(';').filter(|i| *i <= 10) else {
            out.push('&');
            rest = after.get(1..).unwrap_or_default();
            continue;
        };
        let entity = after.get(1..semi).unwrap_or_default();
        let c = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") => u32::from_str_radix(e.get(2..).unwrap_or_default(), 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e.get(1..).and_then(|d| d.parse().ok()).and_then(char::from_u32),
            _ => None,
        };
        match c {
            Some(c) => out.push(c),
            None => out.push_str(after.get(..=semi).unwrap_or_default()),
        }
        rest = after.get(semi.saturating_add(1)..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// The element tags of `xml` in order (comments, declarations and processing instructions are skipped).
fn tags(xml: &str) -> Vec<Tag> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(open) = xml.get(at..).and_then(|s| s.find('<')).map(|i| at.saturating_add(i)) {
        let rest = xml.get(open..).unwrap_or_default();
        if rest.starts_with("<!--") {
            at = rest.find("-->").map_or(xml.len(), |i| open.saturating_add(i).saturating_add(3));
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            at = rest.find('>').map_or(xml.len(), |i| open.saturating_add(i).saturating_add(1));
            continue;
        }
        // The end of the tag: the first `>` outside an attribute value.
        let mut quote = None;
        let mut close = None;
        for (i, c) in rest.char_indices().skip(1) {
            match (quote, c) {
                (None, '"' | '\'') => quote = Some(c),
                (Some(q), c) if c == q => quote = None,
                (None, '>') => {
                    close = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let Some(close) = close else { break };
        let inner = rest.get(1..close).unwrap_or_default();
        let closing = inner.starts_with('/');
        let inner = inner.trim_start_matches('/').trim_end_matches('/');
        let qname: String = inner.chars().take_while(|c| !c.is_whitespace()).collect();
        let name = qname.rsplit(':').next().unwrap_or_default().to_owned();
        let mut attrs = BTreeMap::new();
        let mut a = inner.get(qname.len()..).unwrap_or_default();
        while let Some(eq) = a.find('=') {
            let key = a.get(..eq).unwrap_or_default().trim().to_owned();
            let value_part = a.get(eq.saturating_add(1)..).unwrap_or_default().trim_start();
            let Some(q) = value_part.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
            let Some(end) = value_part.get(1..).and_then(|s| s.find(q)) else { break };
            attrs.insert(key, unescape(value_part.get(1..end.saturating_add(1)).unwrap_or_default()));
            a = value_part.get(end.saturating_add(2)..).unwrap_or_default();
        }
        at = open.saturating_add(close).saturating_add(1);
        out.push(Tag { name, attrs, end: at, closing });
    }
    out
}

/// The text after a tag, up to the next tag.
fn text_after(xml: &str, tag: &Tag) -> String {
    let rest = xml.get(tag.end..).unwrap_or_default();
    unescape(rest.get(..rest.find('<').unwrap_or(rest.len())).unwrap_or_default().trim())
}

// --- running the tools -------------------------------------------------------------------------------------------------------

fn java() -> Option<String> {
    let java = std::env::var("JAVA").unwrap_or_else(|_| "java".to_owned());
    Command::new(&java).arg("-version").output().ok().filter(|o| o.status.success()).map(|_| java)
}

/// The file an environment variable names, when it is set and the file exists.
fn tool(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_file())
}

fn run_java(java: &str, args: &[&std::ffi::OsStr], dir: &Path) -> Result<(bool, String, String)> {
    let out = Command::new(java).args(args).current_dir(dir).output().with_context(|| format!("running {java}"))?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

fn out_dir(args: &[String], default: &str) -> Result<PathBuf> {
    let mut out = std::env::temp_dir().join(default);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = PathBuf::from(it.next().context("--out needs a directory")?),
            other => bail!("unknown option `{other}` (usage: --out DIR)"),
        }
    }
    fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    Ok(out)
}

// --- pdf-oracle --------------------------------------------------------------------------------------------------------------

/// What veraPDF says of one file: compliant or not, the profile it used, and the rules that failed (clause-test: description).
struct VeraVerdict {
    compliant: bool,
    profile: String,
    failed: Vec<String>,
}

/// The verdicts of a veraPDF XML report, by file name.
fn vera_report(xml: &str) -> BTreeMap<String, VeraVerdict> {
    let mut out = BTreeMap::new();
    let tags = tags(xml);
    let mut file = None;
    for (i, t) in tags.iter().enumerate() {
        if t.closing {
            continue;
        }
        match t.name.as_str() {
            "name" => {
                let path = text_after(xml, t);
                let name = Path::new(&path).file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned());
                file = Some(name);
            }
            "validationReport" => {
                if let Some(f) = &file {
                    out.insert(
                        f.clone(),
                        VeraVerdict {
                            compliant: t.attrs.get("isCompliant").is_some_and(|v| v == "true"),
                            profile: t.attrs.get("profileName").cloned().unwrap_or_default(),
                            failed: Vec::new(),
                        },
                    );
                }
            }
            "rule" if t.attrs.get("status").is_some_and(|s| s == "failed") => {
                let description = tags.get(i.saturating_add(1)..).unwrap_or_default().iter().find(|d| d.name == "description" && !d.closing);
                let what = format!(
                    "{}-{} ({} checks): {}",
                    t.attrs.get("clause").map_or("?", String::as_str),
                    t.attrs.get("testNumber").map_or("?", String::as_str),
                    t.attrs.get("failedChecks").map_or("?", String::as_str),
                    description.map(|d| text_after(xml, d)).unwrap_or_default()
                );
                if let Some(v) = file.as_ref().and_then(|f| out.get_mut(f)) {
                    v.failed.push(what);
                }
            }
            _ => {}
        }
    }
    out
}

/// What Mustang's `validate` says of one file: its overall status, and its errors (type: message).
fn mustang_report(xml: &str) -> (Option<String>, Vec<String>) {
    let tags = tags(xml);
    let status = tags.iter().rev().find(|t| t.name == "summary" && !t.closing).and_then(|t| t.attrs.get("status").cloned());
    let errors = tags
        .iter()
        .filter(|t| !t.closing && matches!(t.name.as_str(), "error" | "exception"))
        .map(|t| format!("{} {}: {}", t.name, t.attrs.get("type").map_or("?", String::as_str), text_after(xml, t)))
        .collect();
    (status, errors)
}

pub fn pdf(args: &[String]) -> Result<()> {
    let out = out_dir(args, "xmlcraft-pdf-oracle")?;
    let java = java();
    let vera = tool("VERAPDF");
    let mustang = tool("MUSTANG_JAR");
    let Some(java) = java.filter(|_| vera.is_some() || mustang.is_some()) else {
        println!(
            "pdf-oracle: skipped — set VERAPDF (the veraPDF CLI jar, bin/cli-<version>.jar of a greenfield installation) and/or MUSTANG_JAR \
             (Mustang-CLI-<version>.jar), with java on the PATH or in JAVA; see docs/target-app-parity.md"
        );
        return Ok(());
    };
    // Render the set.
    let mut set = invoices()?;
    let en16931 = read("crates/invoice-pdf/tests/fixtures/en16931-cii.xml")?;
    set.push(Invoice { name: "lines-300".into(), text: long_invoice(&en16931, 300)? });
    set.push(Invoice {
        name: "missing-glyphs".into(),
        text: replace_first(&en16931, "<ram:Name>Fachbuch</ram:Name>", "<ram:Name>Fachbuch 漢字 😀</ram:Name>")?,
    });
    let mut files: Vec<(String, bool)> = Vec::new();
    for invoice in &set {
        for (template, source) in [("de", xmlcraft_invoice_pdf::TemplateSource::De), ("en", xmlcraft_invoice_pdf::TemplateSource::En)] {
            let options = xmlcraft_invoice_pdf::Options { template: source, time: TIME, icc_profile: None };
            let rendered = xmlcraft_engine::einvoice::einvoice_render_pdf(&invoice.text, invoice.text.as_bytes(), &options)
                .map_err(anyhow::Error::msg)?
                .map_err(|e| anyhow::anyhow!("rendering {} ({template}): {e}", invoice.name))?;
            let name = format!("{}-{template}.pdf", invoice.name);
            fs::write(out.join(&name), &rendered.bytes).with_context(|| format!("writing {name}"))?;
            files.push((name, rendered.facturx.hybrid));
        }
    }
    println!("pdf-oracle: rendered {} PDFs into {}", files.len(), out.display());
    let mut failures = 0usize;
    if let Some(vera) = &vera {
        let (_, version, _) = run_java(&java, &["-jar".as_ref(), vera.as_os_str(), "--version".as_ref()], &out)?;
        println!("\nveraPDF: {}", version.lines().next().unwrap_or("?").trim());
        for (label, flavour) in [("PDF/A-3b", Some("3b")), ("auto-detected flavour", None)] {
            let mut a: Vec<&std::ffi::OsStr> = vec!["-jar".as_ref(), vera.as_os_str(), "--format".as_ref(), "xml".as_ref()];
            if let Some(f) = flavour {
                a.push("--flavour".as_ref());
                a.push(f.as_ref());
            }
            a.extend(files.iter().map(|(n, _)| std::ffi::OsStr::new(n.as_str())));
            let (_, report, err) = run_java(&java, &a, &out)?;
            let verdicts = vera_report(&report);
            println!("  {label}:");
            for (name, _) in &files {
                match verdicts.get(name) {
                    Some(v) if v.compliant => println!("    PASS  {name}  ({})", v.profile),
                    Some(v) => {
                        failures = failures.saturating_add(1);
                        println!("    FAIL  {name}  ({})", v.profile);
                        for f in &v.failed {
                            println!("          {f}");
                        }
                    }
                    None => {
                        failures = failures.saturating_add(1);
                        println!("    FAIL  {name}  (no verdict; veraPDF said: {})", err.lines().last().unwrap_or_default());
                    }
                }
            }
        }
    }
    if let Some(mustang) = &mustang {
        let jar = mustang.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        println!("\nMustang ({jar}): validate");
        let mut validator = None;
        for (name, hybrid) in &files {
            let a: Vec<&std::ffi::OsStr> = vec![
                "-jar".as_ref(),
                mustang.as_os_str(),
                "--disable-file-logging".as_ref(),
                "--action".as_ref(),
                "validate".as_ref(),
                "--source".as_ref(),
                name.as_ref(),
            ];
            let (_, report, _) = run_java(&java, &a, &out)?;
            let (status, errors) = mustang_report(&report);
            if validator.is_none() {
                validator = tags(&report).into_iter().find(|t| t.name == "validator").and_then(|t| t.attrs.get("version").cloned());
            }
            match (status.as_deref(), hybrid) {
                (Some("valid"), _) => println!("    PASS  {name}"),
                // Factur-X / ZUGFeRD is CII only: a UBL invoice in a PDF is no hybrid, and Mustang says so.
                (_, false) => println!("    n/a   {name}  (no Factur-X hybrid by design; Mustang: {})", status.as_deref().unwrap_or("no verdict")),
                _ => {
                    failures = failures.saturating_add(1);
                    println!("    FAIL  {name}  ({})", status.as_deref().unwrap_or("no verdict"));
                    for e in &errors {
                        println!("          {e}");
                    }
                }
            }
        }
        println!("  (Mustang validator version {})", validator.as_deref().unwrap_or("not reported"));
    }
    if vera.is_none() {
        println!("\nveraPDF: skipped (VERAPDF is not set)");
    }
    if mustang.is_none() {
        println!("\nMustang: skipped (MUSTANG_JAR is not set)");
    }
    if failures > 0 {
        bail!("{failures} verdicts failed");
    }
    println!("\npdf-oracle: every verdict passed");
    Ok(())
}

// --- einvoice-oracle ---------------------------------------------------------------------------------------------------------

/// A change that breaks an invoice: replace the first occurrence of a text, or remove the first element that starts so.
enum Edit {
    Replace(&'static str, &'static str),
    Remove(&'static str),
}

use Edit::{Remove, Replace};

/// The broken variants of the XRechnung CII invoice (two lines, two VAT rates, an allowance) and of the XRechnung UBL invoice.
const CII_BREAKS: &[(&str, &[Edit])] = &[
    ("line-total", &[Replace("<ram:LineTotalAmount>130.00", "<ram:LineTotalAmount>131.00")]),
    ("grand-total", &[Replace("<ram:GrandTotalAmount>139.20", "<ram:GrandTotalAmount>140.00")]),
    ("tax-total", &[Replace("<ram:TaxTotalAmount currencyID=\"EUR\">19.20", "<ram:TaxTotalAmount currencyID=\"EUR\">19.30")]),
    ("due-payable", &[Replace("<ram:DuePayableAmount>139.20", "<ram:DuePayableAmount>100.00")]),
    ("allowance-total", &[Replace("<ram:AllowanceTotalAmount>10.00", "<ram:AllowanceTotalAmount>12.00")]),
    ("vat-amount", &[Replace("<ram:CalculatedAmount>17.10", "<ram:CalculatedAmount>17.00")]),
    ("no-seller-vat-id", &[Remove("<ram:SpecifiedTaxRegistration>")]),
    ("no-buyer-name", &[Replace("<ram:Name>Beispiel Kunde AG</ram:Name>", "")]),
    ("no-currency", &[Remove("<ram:InvoiceCurrencyCode>")]),
    ("bad-currency", &[Replace("<ram:InvoiceCurrencyCode>EUR", "<ram:InvoiceCurrencyCode>EUX")]),
    ("tax-currency", &[Replace("<ram:TaxTotalAmount currencyID=\"EUR\">", "<ram:TaxTotalAmount currencyID=\"USD\">")]),
    ("no-payment-means", &[Remove("<ram:SpecifiedTradeSettlementPaymentMeans>")]),
    ("bad-payment-means", &[Replace("<ram:TypeCode>58</ram:TypeCode>", "<ram:TypeCode>999</ram:TypeCode>")]),
    ("bad-type-code", &[Replace("<ram:TypeCode>380</ram:TypeCode>", "<ram:TypeCode>999</ram:TypeCode>")]),
    ("bad-country", &[Replace("<ram:CountryID>DE</ram:CountryID>", "<ram:CountryID>XX</ram:CountryID>")]),
    ("line-vat-category", &[Replace("<ram:CategoryCode>S</ram:CategoryCode>", "<ram:CategoryCode>Z</ram:CategoryCode>")]),
    (
        "header-vat-exempt",
        &[Replace(
            "<ram:CategoryCode>S</ram:CategoryCode>\n        <ram:RateApplicablePercent>7",
            "<ram:CategoryCode>E</ram:CategoryCode>\n        <ram:RateApplicablePercent>7",
        )],
    ),
    ("bad-date", &[Replace(">20261101</udt:DateTimeString>", ">20261301</udt:DateTimeString>")]),
    ("bad-date-format", &[Replace("<udt:DateTimeString format=\"102\">20261101", "<udt:DateTimeString format=\"999\">20261101")]),
    ("negative-price", &[Replace("<ram:ChargeAmount>30.00</ram:ChargeAmount>", "<ram:ChargeAmount>-30.00</ram:ChargeAmount>")]),
    ("negative-allowance", &[Replace("<ram:ActualAmount>10.00", "<ram:ActualAmount>-10.00")]),
    ("duplicate-line-id", &[Replace("<ram:LineID>2</ram:LineID>", "<ram:LineID>1</ram:LineID>")]),
    ("allowance-no-reason", &[Remove("<ram:ReasonCode>"), Remove("<ram:Reason>")]),
    ("no-buyer-reference", &[Remove("<ram:BuyerReference>")]),
    ("no-seller-contact", &[Remove("<ram:DefinedTradeContact>")]),
    ("bad-unit", &[Replace("unitCode=\"HUR\"", "unitCode=\"XYZ\"")]),
    ("no-issue-date", &[Remove("<ram:IssueDateTime>")]),
    ("bad-iban", &[Replace("<ram:IBANID>DE02120300000000202051", "<ram:IBANID>XX")]),
    ("bad-vat-scheme", &[Replace("<ram:ID schemeID=\"VA\">DE123456789", "<ram:ID schemeID=\"XX\">DE123456789")]),
    ("bad-amount-format", &[Replace("<ram:BasisAmount>90.00</ram:BasisAmount>", "<ram:BasisAmount>90.001</ram:BasisAmount>")]),
];

const UBL_BREAKS: &[(&str, &[Edit])] = &[
    (
        "line-total",
        &[Replace(
            "<cbc:LineExtensionAmount currencyID=\"EUR\">100.00</cbc:LineExtensionAmount>\n    <cbc:TaxExclusiveAmount",
            "<cbc:LineExtensionAmount currencyID=\"EUR\">101.00</cbc:LineExtensionAmount>\n    <cbc:TaxExclusiveAmount",
        )],
    ),
    ("payable", &[Replace("<cbc:PayableAmount currencyID=\"EUR\">119.00", "<cbc:PayableAmount currencyID=\"EUR\">120.00")]),
    (
        "tax-total",
        &[Replace(
            "<cbc:TaxAmount currencyID=\"EUR\">19.00</cbc:TaxAmount>\n    <cac:TaxSubtotal>",
            "<cbc:TaxAmount currencyID=\"EUR\">19.50</cbc:TaxAmount>\n    <cac:TaxSubtotal>",
        )],
    ),
    ("no-seller-vat-id", &[Remove("<cac:PartyTaxScheme>")]),
    ("no-currency", &[Remove("<cbc:DocumentCurrencyCode>")]),
    ("bad-currency", &[Replace("<cbc:DocumentCurrencyCode>EUR", "<cbc:DocumentCurrencyCode>EUX")]),
    ("amount-currency", &[Replace("<cbc:PayableAmount currencyID=\"EUR\">", "<cbc:PayableAmount currencyID=\"USD\">")]),
    ("no-payment-means", &[Remove("<cac:PaymentMeans>")]),
    ("bad-payment-means", &[Replace("<cbc:PaymentMeansCode>58", "<cbc:PaymentMeansCode>999")]),
    ("bad-type-code", &[Replace("<cbc:InvoiceTypeCode>380", "<cbc:InvoiceTypeCode>999")]),
    ("bad-country", &[Replace("<cbc:IdentificationCode>DE</cbc:IdentificationCode>", "<cbc:IdentificationCode>XX</cbc:IdentificationCode>")]),
    (
        "line-vat-category",
        &[Replace("<cac:ClassifiedTaxCategory>\n        <cbc:ID>S</cbc:ID>", "<cac:ClassifiedTaxCategory>\n        <cbc:ID>E</cbc:ID>")],
    ),
    ("bad-date", &[Replace("<cbc:IssueDate>2026-11-01", "<cbc:IssueDate>2026-13-01")]),
    ("due-before-issue", &[Replace("<cbc:DueDate>2026-12-01", "<cbc:DueDate>2026-10-01")]),
    ("negative-price", &[Replace("<cbc:PriceAmount currencyID=\"EUR\">50.00", "<cbc:PriceAmount currencyID=\"EUR\">-50.00")]),
    ("no-buyer-reference", &[Remove("<cbc:BuyerReference>")]),
    ("bad-unit", &[Replace("unitCode=\"C62\"", "unitCode=\"XYZ\"")]),
    ("no-seller-endpoint", &[Remove("<cbc:EndpointID schemeID=\"EM\">rechnung@example.com")]),
    ("no-seller-contact", &[Remove("<cac:Contact>")]),
    ("no-buyer-name", &[Remove("<cac:PartyLegalEntity>\n        <cbc:RegistrationName>Beispiel Amt")]),
    (
        "line-amount",
        &[Replace(
            "<cbc:LineExtensionAmount currencyID=\"EUR\">100.00</cbc:LineExtensionAmount>\n    <cac:Item>",
            "<cbc:LineExtensionAmount currencyID=\"EUR\">99.00</cbc:LineExtensionAmount>\n    <cac:Item>",
        )],
    ),
    ("bad-vat-rate", &[Replace("<cbc:Percent>19</cbc:Percent>", "<cbc:Percent>-19</cbc:Percent>")]),
];

fn broken(base: &str, edits: &[Edit]) -> Result<String> {
    let mut text = base.to_owned();
    for e in edits {
        text = match e {
            Replace(from, to) => replace_first(&text, from, to)?,
            Remove(open) => remove_element(&text, open)?,
        };
    }
    Ok(text)
}

/// A location (an XPath to a node, as `fn:path` writes it or as the ISO Schematron skeleton does) without its namespaces:
/// `/*:Invoice[namespace-uri()='…'][1]/*:ID[namespace-uri()='…'][1]` and `/Q{…}Invoice[1]/Q{…}ID[1]` both become `/Invoice[1]/ID[1]`.
fn normalize_location(location: &str) -> String {
    let mut out = String::new();
    for step in location.split('/').filter(|s| !s.is_empty()) {
        let mut s = step.to_owned();
        while let Some(i) = s.find("[namespace-uri()=") {
            let end = s.get(i..).and_then(|r| r.find(']')).map_or(s.len(), |e| i.saturating_add(e).saturating_add(1));
            s = format!("{}{}", s.get(..i).unwrap_or_default(), s.get(end..).unwrap_or_default());
        }
        if let Some(i) = s.find("local-name()='") {
            let name: String = s.get(i.saturating_add(14)..).unwrap_or_default().chars().take_while(|c| *c != '\'').collect();
            let index = s.rfind('[').filter(|j| s.get(*j..).is_some_and(|r| r.trim_start_matches('[').starts_with(|c: char| c.is_ascii_digit())));
            let prefix = if s.starts_with('@') { "@" } else { "" };
            s = format!("{prefix}{name}{}", index.and_then(|j| s.get(j..)).unwrap_or_default());
        }
        let (at, rest) = match s.strip_prefix('@') {
            Some(r) => ("@", r.to_owned()),
            None => ("", s),
        };
        let rest = match rest.strip_prefix("Q{") {
            Some(r) => r.get(r.find('}').map_or(0, |i| i.saturating_add(1))..).unwrap_or_default().to_owned(),
            None => rest,
        };
        let rest = rest.strip_prefix("*:").map(str::to_owned).unwrap_or(rest);
        let name_end = rest.find('[').unwrap_or(rest.len());
        let rest = match rest.get(..name_end).and_then(|n| n.rfind(':')) {
            Some(colon) => rest.get(colon.saturating_add(1)..).unwrap_or_default().to_owned(),
            None => rest,
        };
        out.push('/');
        out.push_str(at);
        out.push_str(&rest);
    }
    out
}

/// The failed assertions and successful reports of an SVRL report: (rule id, normalized location, flag).
fn svrl_findings(xml: &str) -> Vec<(String, String)> {
    tags(xml)
        .into_iter()
        .filter(|t| !t.closing && matches!(t.name.as_str(), "failed-assert" | "successful-report"))
        .map(|t| (t.attrs.get("id").cloned().unwrap_or_else(|| "?".into()), normalize_location(t.attrs.get("location").map_or("", String::as_str))))
        .collect()
}

/// The rule sets compiled to XSLT, found by their file names under `dir`.
fn find_xslt(dir: &Path, names: &[&str]) -> BTreeMap<String, PathBuf> {
    let mut found = BTreeMap::new();
    let mut todo = vec![(dir.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((d, depth)) = todo.pop() {
        let Ok(entries) = fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            seen = seen.saturating_add(1);
            if seen > 100_000 {
                return found;
            }
            let p = e.path();
            if p.is_dir() && depth < 8 {
                todo.push((p, depth.saturating_add(1)));
            } else if let Some(n) = p.file_name().and_then(|n| n.to_str()).filter(|n| names.contains(n)) {
                found.entry(n.to_owned()).or_insert(p);
            }
        }
    }
    found
}

/// Multiset difference: what `a` has more of than `b`.
fn minus(a: &[(String, String)], b: &[(String, String)]) -> Vec<(String, String)> {
    let mut left: BTreeMap<&(String, String), usize> = BTreeMap::new();
    for x in b {
        *left.entry(x).or_default() += 1;
    }
    let mut out = Vec::new();
    for x in a {
        match left.get_mut(x) {
            Some(n) if *n > 0 => *n -= 1,
            _ => out.push(x.clone()),
        }
    }
    out
}

pub fn einvoice(args: &[String]) -> Result<()> {
    let out = out_dir(args, "xmlcraft-einvoice-oracle")?;
    let (Some(java), Some(saxon), Some(xslt_dir)) = (java(), std::env::var_os("SAXON_JAR"), std::env::var_os("EINVOICE_XSLT_DIR").map(PathBuf::from))
    else {
        println!(
            "einvoice-oracle: skipped — set SAXON_JAR (Saxon-HE's jar, or a class path with the xmlresolver jars it needs) and \
             EINVOICE_XSLT_DIR (the unpacked CEN EN 16931 and KoSIT XRechnung release archives), with java on the PATH or in JAVA; \
             see docs/target-app-parity.md"
        );
        return Ok(());
    };
    const SETS: [(&str, &str); 4] = [
        ("cii-en16931", "EN16931-CII-validation.xslt"),
        ("ubl-en16931", "EN16931-UBL-validation.xslt"),
        ("cii-cius", "XRechnung-CII-validation.xsl"),
        ("ubl-cius", "XRechnung-UBL-validation.xsl"),
    ];
    let xslt = find_xslt(&xslt_dir, &SETS.map(|(_, f)| f));
    for (_, f) in SETS {
        if !xslt.contains_key(f) {
            bail!("{f} is not under {} (unpack the release archives there)", xslt_dir.display());
        }
    }
    // The invoices: the well-formed ones and the broken variants of the XRechnung invoice of each syntax.
    let mut set = invoices()?;
    let cii_base = with_guideline(&read("crates/invoice-pdf/tests/fixtures/en16931-cii.xml")?, XRECHNUNG_ID)?;
    let ubl_base = read("crates/einvoice/tests/fixtures/xrechnung-ubl.xml")?;
    for (name, edits) in CII_BREAKS {
        set.push(Invoice { name: format!("cii-{name}"), text: broken(&cii_base, edits)? });
    }
    for (name, edits) in UBL_BREAKS {
        set.push(Invoice { name: format!("ubl-{name}"), text: broken(&ubl_base, edits)? });
    }
    // Ours, and which rule sets apply to each invoice.
    let mut ours: BTreeMap<(String, &str), Vec<(String, String)>> = BTreeMap::new();
    let mut not_evaluated: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (dir, _) in SETS {
        let d = out.join(dir);
        if d.exists() {
            fs::remove_dir_all(&d).with_context(|| format!("clearing {}", d.display()))?;
        }
        fs::create_dir_all(out.join(format!("{dir}-svrl"))).with_context(|| format!("creating {dir}-svrl"))?;
        fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
    }
    for invoice in &set {
        let report = xmlcraft_engine::einvoice::einvoice_validate(&invoice.text, "", Some(vec![Stage::En16931, Stage::Cius]), MAX_FINDINGS)
            .map_err(anyhow::Error::msg)?;
        let syntax = if invoice.text.contains("CrossIndustryInvoice:100") { "cii" } else { "ubl" };
        for (stage, key) in [(Stage::En16931, "en16931"), (Stage::Cius, "cius")] {
            let ran = report.stage(stage).is_some_and(|s| !matches!(s.status, Status::NotRun(_)));
            if !ran {
                continue;
            }
            let dir = format!("{syntax}-{key}");
            let set_key = SETS.iter().map(|(d, _)| *d).find(|d| *d == dir).context("a rule set")?;
            fs::write(out.join(&dir).join(format!("{}.xml", invoice.name)), &invoice.text).with_context(|| format!("writing {}", invoice.name))?;
            let found: Vec<(String, String)> = report
                .findings
                .iter()
                .filter(|f| f.stage == stage)
                .map(|f| (f.rule.clone().unwrap_or_else(|| "?".into()), normalize_location(f.path.as_deref().unwrap_or_default())))
                .collect();
            ours.insert((invoice.name.clone(), set_key), found);
        }
        if report.not_evaluated_total > 0 {
            not_evaluated
                .entry(invoice.name.clone())
                .or_default()
                .extend(report.not_evaluated.iter().map(|n| format!("{} ({})", n.rule.as_deref().unwrap_or("?"), n.reason)));
        }
    }
    // Saxon, once per rule set over the directory of the invoices it applies to.
    let mut theirs: BTreeMap<(String, &str), Vec<(String, String)>> = BTreeMap::new();
    let mut saxon_version = String::new();
    for (dir, file) in SETS {
        let Some(sheet) = xslt.get(file) else { continue };
        let source = out.join(dir);
        let target = out.join(format!("{dir}-svrl"));
        let a: Vec<std::ffi::OsString> = vec![
            "-cp".into(),
            saxon.clone(),
            "net.sf.saxon.Transform".into(),
            "-t".into(),
            format!("-s:{}", source.display()).into(),
            format!("-xsl:{}", sheet.display()).into(),
            format!("-o:{}", target.display()).into(),
        ];
        let refs: Vec<&std::ffi::OsStr> = a.iter().map(|s| s.as_os_str()).collect();
        let (ok, _, err) = run_java(&java, &refs, &out)?;
        if saxon_version.is_empty() {
            saxon_version = err.lines().find(|l| l.contains("Saxon")).unwrap_or_default().trim().to_owned();
        }
        if !ok {
            bail!("Saxon failed on {dir}: {}", err.lines().rev().take(5).collect::<Vec<_>>().join(" / "));
        }
        for ((name, set_key), _) in ours.iter().filter(|((_, k), _)| *k == dir) {
            let svrl = fs::read_to_string(target.join(format!("{name}.xml"))).with_context(|| format!("reading the SVRL of {name}"))?;
            theirs.insert((name.clone(), set_key), svrl_findings(&svrl));
        }
    }
    // The comparison.
    println!("einvoice-oracle: {} invoices, {} rule-set runs; Saxon: {saxon_version}", set.len(), ours.len());
    let mut disagreements = 0usize;
    let mut agreeing_runs = 0usize;
    let mut rules: BTreeMap<String, usize> = BTreeMap::new();
    for (key, mine) in &ours {
        let saxon = theirs.get(key).cloned().unwrap_or_default();
        for (id, _) in mine.iter().chain(&saxon) {
            *rules.entry(id.clone()).or_default() += 1;
        }
        let only_ours = minus(mine, &saxon);
        let only_saxon = minus(&saxon, mine);
        let (name, set_key) = key;
        if only_ours.is_empty() && only_saxon.is_empty() {
            agreeing_runs = agreeing_runs.saturating_add(1);
            let ids: Vec<&str> = saxon.iter().map(|(i, _)| i.as_str()).collect();
            println!("  agree     {name:<28} {set_key:<12} {}", if ids.is_empty() { "(no findings)".to_owned() } else { ids.join(" ") });
        } else {
            disagreements = disagreements.saturating_add(only_ours.len()).saturating_add(only_saxon.len());
            println!("  DISAGREE  {name:<28} {set_key}");
            for (id, at) in &only_ours {
                println!("            only XmlCraft: {id} at {at}");
            }
            for (id, at) in &only_saxon {
                println!("            only Saxon:    {id} at {at}");
            }
        }
    }
    for (name, list) in &not_evaluated {
        println!("  not evaluated by XmlCraft in {name}: {}", list.join("; "));
    }
    println!(
        "\neinvoice-oracle: {agreeing_runs} of {} rule-set runs agree (rule id and location); {} distinct rules fired; {disagreements} \
         disagreeing findings; {} invoices with checks XmlCraft could not evaluate",
        ours.len(),
        rules.len(),
        not_evaluated.len()
    );
    if disagreements > 0 || !not_evaluated.is_empty() {
        bail!("XmlCraft and Saxon disagree");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_from_both_writers_compare_equal() {
        let skeleton = "/*:Invoice[namespace-uri()='urn:oasis:names:specification:ubl:schema:xsd:Invoice-2'][1]/*:LegalMonetaryTotal[namespace-uri()='urn:x'][1]/@currencyID";
        let path = "/Q{urn:oasis:names:specification:ubl:schema:xsd:Invoice-2}Invoice[1]/Q{urn:x}LegalMonetaryTotal[1]/@currencyID";
        assert_eq!(normalize_location(skeleton), "/Invoice[1]/LegalMonetaryTotal[1]/@currencyID");
        assert_eq!(normalize_location(path), normalize_location(skeleton));
        assert_eq!(normalize_location("/rsm:CrossIndustryInvoice[1]/@*[local-name()='unitCode'][1]"), "/CrossIndustryInvoice[1]/@unitCode[1]");
        assert_eq!(normalize_location(""), "");
    }

    #[test]
    fn report_tags_are_read_with_their_attributes_across_lines() {
        let xml = "<?xml version=\"1.0\"?><!-- c --><svrl:schematron-output>\n<svrl:failed-assert test=\"a &gt; b\"\n   id=\"BR-1\" location='/x[1]'>\n<svrl:text>t &amp; u</svrl:text></svrl:failed-assert><svrl:successful-report id=\"R\" location=\"/Q{u}y[1]\"/></svrl:schematron-output>";
        assert_eq!(svrl_findings(xml), vec![("BR-1".into(), "/x[1]".into()), ("R".into(), "/y[1]".into())]);
        let t = tags(xml);
        assert!(t.iter().any(|t| t.name == "failed-assert" && t.attrs.get("test").map(String::as_str) == Some("a > b")));
        assert!(tags("<a b=\"unterminated").is_empty());
        assert_eq!(unescape("&#x41;&#66;&bogus;&"), "AB&bogus;&");
    }

    #[test]
    fn verapdf_and_mustang_reports_are_read() {
        let vera = "<report><jobs><job><item><name>/t/a.pdf</name></item><validationReport profileName=\"PDF/A-3b validation profile\" isCompliant=\"false\"><details><rule clause=\"6.2.11.8\" testNumber=\"1\" status=\"failed\" failedChecks=\"2\"><description>No .notdef</description></rule><rule clause=\"6.1\" status=\"passed\"/></details></validationReport></job><job><item><name>/t/b.pdf</name></item><validationReport isCompliant=\"true\"/></job></jobs></report>";
        let r = vera_report(vera);
        assert!(!r["a.pdf"].compliant && r["b.pdf"].compliant);
        assert_eq!(r["a.pdf"].failed, vec!["6.2.11.8-1 (2 checks): No .notdef".to_owned()]);
        let mustang = "<validation><pdf><messages><error type=\"11\">XMP: missing</error></messages><summary status=\"invalid\"/></pdf><summary status=\"invalid\"/></validation>";
        assert_eq!(mustang_report(mustang), (Some("invalid".into()), vec!["error 11: XMP: missing".into()]));
    }

    #[test]
    fn the_fixture_set_and_its_broken_variants_are_made() {
        let set = invoices().expect("fixtures");
        assert_eq!(set.len(), 9);
        let basic = set.iter().find(|i| i.name == "basic").expect("basic");
        assert!(basic.text.contains("factur-x.eu:1p0:basic") && !basic.text.contains("DefinedTradeContact"));
        let cii = with_guideline(&read("crates/invoice-pdf/tests/fixtures/en16931-cii.xml").expect("read"), XRECHNUNG_ID).expect("xr");
        let ubl = read("crates/einvoice/tests/fixtures/xrechnung-ubl.xml").expect("read");
        for (name, edits) in CII_BREAKS {
            let t = broken(&cii, edits).unwrap_or_else(|e| panic!("cii-{name}: {e}"));
            assert_ne!(t, cii, "cii-{name}");
        }
        for (name, edits) in UBL_BREAKS {
            let t = broken(&ubl, edits).unwrap_or_else(|e| panic!("ubl-{name}: {e}"));
            assert_ne!(t, ubl, "ubl-{name}");
        }
        let long = long_invoice(&cii, 3).expect("long");
        assert_eq!(long.matches("<ram:LineID>").count(), 3);
        assert!(long.contains("<ram:GrandTotalAmount>35.70</ram:GrandTotalAmount>"));
        assert!(remove_element("<a>\n  <b>x</b>\n</a>", "<c>").is_err());
        assert_eq!(remove_element("<a>\n  <b>x</b>\n</a>", "<b>").expect("removed"), "<a>\n</a>");
    }
}
