#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! The visual rendering of an electronic invoice: a PDF that shows the invoice to people and carries the original XML unchanged, so
//! that a UN/CEFACT CII invoice becomes a ZUGFeRD 2.x / Factur-X hybrid invoice.
//!
//! - **Layer:** L4, no UI and no I/O (the caller reads and writes files), so it compiles for the web.
//! - The invoice is read into the neutral model of `xmlcraft-einvoice`; a [`template`] says what the pages show; [`layout`] places
//!   it; [`pdf`] writes it with PdfCraft's object layer (`pdfcraft-cos`), with the Inter faces embedded as subsets.
//! - The XML is embedded byte for byte. A CII invoice with a Factur-X profile (or the XRECHNUNG reference profile) is embedded as
//!   `factur-x.xml` (`xrechnung.xml` for XRECHNUNG) with the Factur-X XMP; a UBL invoice is attached as a plain associated file
//!   (Factur-X is CII only).
//! - PDF/A-3: the file is written to the parts of ISO 19005-3 XmlCraft can meet (embedded fonts, XMP, associated file, no
//!   encryption), but a PDF/A output intent needs an ICC profile and XmlCraft ships none, so the file says it is PDF/A-3b only when
//!   the caller gives a suitable profile. Neither PDF/A nor Factur-X conformance is verified here (no veraPDF, no Mustang).

pub mod font;
pub mod format;
pub mod layout;
pub mod pdf;
pub mod template;

use xmlcraft_einvoice::model::{self, Invoice};
use xmlcraft_einvoice::{Standard, Syntax, detect};

pub use template::TemplateError;

/// Which template to use.
#[derive(Clone, Copy, Debug)]
pub enum TemplateSource<'a> {
    De,
    En,
    /// The text of a template.
    Text(&'a str),
}

pub struct Options<'a> {
    pub template: TemplateSource<'a>,
    /// The time the file is made, in seconds since 1970 (UTC).
    pub time: i64,
    /// An ICC output profile for the PDF/A output intent (none ships with XmlCraft).
    pub icc_profile: Option<&'a [u8]>,
}

/// How the invoice XML is embedded and whether that makes a Factur-X hybrid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacturX {
    /// Whether the file is written as a Factur-X / ZUGFeRD hybrid (CII with a known profile, PDF/A-3 identified). Not verified.
    pub hybrid: bool,
    /// The Factur-X conformance level (`MINIMUM`, `BASIC WL`, `BASIC`, `EN 16931`, `EXTENDED`, `XRECHNUNG`).
    pub profile: Option<String>,
    /// The name the XML is embedded under.
    pub file_name: String,
    /// The `/AFRelationship`.
    pub relationship: String,
    /// Why it is not a hybrid, when it is not.
    pub reason: Option<String>,
}

#[derive(Debug)]
pub struct Rendered {
    pub bytes: Vec<u8>,
    pub pages: usize,
    pub warnings: Vec<String>,
    pub facturx: FacturX,
    /// Whether the file identifies itself as PDF/A-3b (it has an output intent). Not verified.
    pub pdfa: bool,
    /// Why it does not, when it does not.
    pub pdfa_reason: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The document is no invoice this renders (and why).
    NotAnInvoice(String),
    Template(TemplateError),
    /// The invoice does not fit (too many pages) or the template leaves no room.
    Layout(String),
    /// The ICC profile cannot be an output intent.
    Icc(String),
    Pdf(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::NotAnInvoice(why) => f.write_str(why),
            RenderError::Template(e) => write!(f, "{e}"),
            RenderError::Layout(why) => write!(f, "the invoice cannot be laid out: {why}"),
            RenderError::Icc(why) => write!(f, "the ICC profile cannot be used: {why}"),
            RenderError::Pdf(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for RenderError {}

/// The Factur-X conformance level of a CII invoice, from what its guideline identifier says (Factur-X 1.07.2 §6.3.1).
fn facturx_level(xml: &str) -> Option<&'static str> {
    let d = detect(xml)?;
    if d.syntax != Syntax::Cii {
        return None;
    }
    match d.standard {
        Standard::XRechnung => Some("XRECHNUNG"),
        Standard::En16931 => Some("EN 16931"),
        Standard::FacturX | Standard::Zugferd2 => match d.profile.as_deref()? {
            "minimum" => Some("MINIMUM"),
            "basic WL" => Some("BASIC WL"),
            "basic" => Some("BASIC"),
            "EN 16931" => Some("EN 16931"),
            "extended" => Some("EXTENDED"),
            _ => None,
        },
        _ => None,
    }
}

/// Render the invoice `xml`, whose original bytes `bytes` are embedded unchanged.
pub fn render(xml: &str, bytes: &[u8], options: &Options<'_>) -> Result<Rendered, RenderError> {
    let invoice: Invoice = model::read(xml)
        .ok_or_else(|| RenderError::NotAnInvoice("the document is not a UBL Invoice or CreditNote or a UN/CEFACT CII CrossIndustryInvoice".into()))?;
    let template = template::read(match options.template {
        TemplateSource::De => template::DE,
        TemplateSource::En => template::EN,
        TemplateSource::Text(t) => t,
    })
    .map_err(RenderError::Template)?;
    let icc = match options.icc_profile {
        Some(p) => Some((p, pdf::icc_components(p).map_err(RenderError::Icc)?)),
        None => None,
    };
    let regular = font::Font::load(font::Face::Regular).map_err(|e| RenderError::Pdf(e.0))?;
    let bold = font::Font::load(font::Face::Bold).map_err(|e| RenderError::Pdf(e.0))?;
    let laid = layout::lay_out(&template, &invoice, &regular, &bold).map_err(RenderError::Layout)?;

    // How the XML is embedded: Factur-X 1.07.2 §6.2 (file name: factur-x.xml, xrechnung.xml for the XRECHNUNG reference profile) and
    // the table of §6.2.2 (relationship: Data for MINIMUM and BASIC WL, whose XML holds less than the page; Alternative for BASIC,
    // EN 16931, EXTENDED and XRECHNUNG, as required in Germany and allowed in France).
    let level = (invoice.syntax == Syntax::Cii).then(|| facturx_level(xml)).flatten();
    let (file_name, relationship) = match level {
        Some("XRECHNUNG") => ("xrechnung.xml", "Alternative"),
        Some("MINIMUM" | "BASIC WL") => ("factur-x.xml", "Data"),
        Some(_) => ("factur-x.xml", "Alternative"),
        // Not Factur-X: the page was made from the XML, which is its source (ISO 32000-2 §14.13, `Source`).
        None => ("invoice.xml", "Source"),
    };
    let reason = match (level, icc.is_some()) {
        (Some(_), true) => None,
        (Some(_), false) => {
            Some("Factur-X needs PDF/A-3, and a PDF/A output intent needs an ICC profile, which XmlCraft does not ship: give icc_profile".to_owned())
        }
        (None, _) if invoice.syntax == Syntax::Ubl => {
            Some("Factur-X / ZUGFeRD embeds UN/CEFACT CII only; the UBL invoice is attached as a plain associated file".to_owned())
        }
        (None, _) => Some("the CII invoice names no Factur-X profile and no XRechnung or EN 16931 specification".to_owned()),
    };
    let number = invoice.number.as_ref().map(|n| n.text.as_str()).unwrap_or_default();
    let title = match template.lang {
        template::Lang::De => format!("Rechnung {number}"),
        template::Lang::En => format!("Invoice {number}"),
    };
    let id_hex = xmlcraft_pdf::md5_hex(&[bytes, title.as_bytes(), &options.time.to_be_bytes()].concat());
    let mut id = [0u8; 16];
    for (i, slot) in id.iter_mut().enumerate() {
        *slot = id_hex.get(i * 2..i * 2 + 2).and_then(|h| u8::from_str_radix(h, 16).ok()).unwrap_or(0);
    }
    let meta = pdf::Meta {
        title: &title,
        lang: match template.lang {
            template::Lang::De => "de",
            template::Lang::En => "en",
        },
        time: options.time,
        facturx: level.map(|l| pdf::FacturXMeta { level: l, file_name }),
        icc,
        id,
    };
    let description = if level.is_some() { "Factur-X invoice" } else { "The invoice as XML" };
    let attachment = pdf::Attachment { name: file_name, data: bytes, relationship, description };
    let out = pdf::write(&laid, &attachment, &meta).map_err(RenderError::Pdf)?;
    Ok(Rendered {
        bytes: out,
        pages: laid.pages.len(),
        warnings: laid.warnings,
        facturx: FacturX {
            hybrid: level.is_some() && icc.is_some(),
            profile: level.map(str::to_owned),
            file_name: file_name.to_owned(),
            relationship: relationship.to_owned(),
            reason,
        },
        pdfa: icc.is_some(),
        pdfa_reason: icc
            .is_none()
            .then(|| "no ICC output profile was given (XmlCraft ships none), so the file does not say it is PDF/A-3".to_owned()),
    })
}
