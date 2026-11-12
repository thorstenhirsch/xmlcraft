//! The invoice PDF, read back by XmlCraft's own PDF reader: the embedded XML is byte for byte the source, named and related as
//! Factur-X says, the XMP carries the Factur-X fields, and the pages show the invoice; a long invoice flows over pages with its header.

use xmlcraft_invoice_pdf::layout::{Item, lay_out};
use xmlcraft_invoice_pdf::{Options, RenderError, TemplateSource, font, render, template};
use xmlcraft_pdf::{Object, Pdf};

const CII: &str = include_str!("../../einvoice/tests/fixtures/xrechnung-cii.xml");
const UBL: &str = include_str!("../../einvoice/tests/fixtures/xrechnung-ubl.xml");
/// 2026-11-12 00:00:00 UTC.
const TIME: i64 = 1_794_441_600;

fn options(template: TemplateSource<'_>) -> Options<'_> {
    Options { template, time: TIME, icc_profile: None }
}

/// The catalog's `/Metadata` as text, and the decoded content streams of the pages.
fn metadata_and_pages(pdf: &Pdf) -> (String, Vec<Vec<u8>>) {
    let root = pdf.resolve(pdf.trailer().get("Root").expect("a root"));
    let catalog = root.as_dict().expect("a catalog").clone();
    let meta = match pdf.resolve(catalog.get("Metadata").expect("metadata")) {
        Object::Stream(s) => String::from_utf8(pdf.stream_data(&s).expect("decoded")).expect("UTF-8"),
        other => panic!("{other:?}"),
    };
    let pages = pdf.resolve(catalog.get("Pages").expect("pages"));
    let kids = pages.as_dict().and_then(|d| d.get("Kids")).and_then(Object::as_array).expect("kids").to_vec();
    let contents = kids
        .iter()
        .map(|k| {
            let page = pdf.resolve(k);
            match pdf.resolve(page.as_dict().and_then(|d| d.get("Contents")).expect("contents")) {
                Object::Stream(s) => pdf.stream_data(&s).expect("decoded"),
                other => panic!("{other:?}"),
            }
        })
        .collect();
    (meta, contents)
}

/// Whether the content stream draws `text` in the regular face (its glyph ids, as the writer writes them).
fn draws(content: &[u8], text: &str, face: font::Face) -> bool {
    let f = font::Font::load(face).expect("font");
    let hex: String = f.glyph_bytes(text).iter().map(|b| format!("{b:02X}")).collect();
    String::from_utf8_lossy(content).contains(&hex)
}

#[test]
fn a_cii_xrechnung_becomes_a_pdf_with_its_xml_embedded_byte_for_byte() {
    let out = render(CII, CII.as_bytes(), &options(TemplateSource::De)).expect("renders");
    assert_eq!(out.pages, 1);
    assert_eq!(out.facturx.profile.as_deref(), Some("XRECHNUNG"));
    assert_eq!((out.facturx.file_name.as_str(), out.facturx.relationship.as_str()), ("xrechnung.xml", "Alternative"));
    assert!(!out.pdfa && !out.facturx.hybrid, "no ICC profile ships, so no PDF/A claim");
    assert!(out.facturx.reason.as_deref().is_some_and(|r| r.contains("ICC")), "{:?}", out.facturx.reason);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let pdf = Pdf::open(out.bytes.clone()).expect("XmlCraft reads it");
    assert!(!pdf.recovered(), "the cross-reference is right");
    let files = pdf.embedded_files();
    assert_eq!(files.len(), 1, "{files:?}");
    let file = &files[0];
    assert_eq!((file.name.as_str(), file.relationship.as_deref(), file.mime.as_deref()), ("xrechnung.xml", Some("Alternative"), Some("text/xml")));
    assert_eq!(pdf.embedded_data(file).expect("data"), CII.as_bytes(), "byte for byte");
    assert_eq!(file.size, Some(CII.len() as u64));
    let (meta, pages) = metadata_and_pages(&pdf);
    for needle in [
        "<fx:DocumentType>INVOICE</fx:DocumentType>",
        "<fx:DocumentFileName>xrechnung.xml</fx:DocumentFileName>",
        "<fx:Version>1.0</fx:Version>",
        "<fx:ConformanceLevel>XRECHNUNG</fx:ConformanceLevel>",
        "urn:factur-x:pdfa:CrossIndustryDocument:invoice:1p0#",
        "<pdfaSchema:prefix>fx</pdfaSchema:prefix>",
        "Rechnung RE-2026-0001",
    ] {
        assert!(meta.contains(needle), "{needle} in {meta}");
    }
    assert!(!meta.contains("pdfaid:part"), "no PDF/A claim without an output intent");
    assert!(draws(&pages[0], "RE-2026-0001", font::Face::Regular), "the invoice number is on the page");
    assert!(draws(&pages[0], "Beispiel Amt", font::Face::Regular), "the buyer is on the page");
    assert!(draws(&pages[0], "119,00 EUR", font::Face::Bold), "the amount due, in bold");
    assert!(draws(&pages[0], "01.11.2026", font::Face::Regular), "the issue date in German form");
    assert!(draws(&pages[0], "Seite 1 von 1", font::Face::Regular), "the footer");
}

#[test]
fn with_an_output_profile_the_file_says_it_is_pdf_a_3b() {
    // A synthetic gray display profile header: enough for the output intent's checks (the profile is not verified further).
    let mut icc = vec![0u8; 132];
    icc[0..4].copy_from_slice(&132u32.to_be_bytes());
    icc[8] = 2;
    icc[12..16].copy_from_slice(b"mntr");
    icc[16..20].copy_from_slice(b"GRAY");
    icc[36..40].copy_from_slice(b"acsp");
    let out = render(CII, CII.as_bytes(), &Options { template: TemplateSource::En, time: TIME, icc_profile: Some(&icc) }).expect("renders");
    assert!(out.pdfa && out.facturx.hybrid && out.facturx.reason.is_none());
    let pdf = Pdf::open(out.bytes).expect("reads");
    let (meta, pages) = metadata_and_pages(&pdf);
    assert!(meta.contains("<pdfaid:part>3</pdfaid:part>") && meta.contains("<pdfaid:conformance>B</pdfaid:conformance>"), "{meta}");
    assert!(draws(&pages[0], "Invoice RE-2026-0001", font::Face::Bold), "the English title");
    let root = pdf.resolve(pdf.trailer().get("Root").expect("root"));
    assert!(root.as_dict().and_then(|d| d.get("OutputIntents")).is_some());
    // A scanner profile is no output intent.
    icc[12..16].copy_from_slice(b"scnr");
    let e = render(CII, CII.as_bytes(), &Options { template: TemplateSource::En, time: TIME, icc_profile: Some(&icc) }).expect_err("refused");
    assert!(matches!(&e, RenderError::Icc(why) if why.contains("scnr")), "{e:?}");
}

#[test]
fn a_ubl_invoice_is_attached_as_a_plain_associated_file() {
    let out = render(UBL, UBL.as_bytes(), &options(TemplateSource::En)).expect("renders");
    assert_eq!((out.facturx.profile, out.facturx.hybrid), (None, false));
    assert!(out.facturx.reason.as_deref().is_some_and(|r| r.contains("CII only")));
    let pdf = Pdf::open(out.bytes).expect("reads");
    let files = pdf.embedded_files();
    assert_eq!(files.len(), 1);
    assert_eq!((files[0].name.as_str(), files[0].relationship.as_deref()), ("invoice.xml", Some("Source")));
    assert_eq!(pdf.embedded_data(&files[0]).expect("data"), UBL.as_bytes());
    let (meta, _) = metadata_and_pages(&pdf);
    assert!(!meta.contains("fx:"), "no Factur-X metadata for UBL");
}

#[test]
fn the_same_input_gives_the_same_file() {
    let a = render(CII, CII.as_bytes(), &options(TemplateSource::De)).expect("renders");
    let b = render(CII, CII.as_bytes(), &options(TemplateSource::De)).expect("renders");
    assert_eq!(a.bytes, b.bytes);
}

/// The CII fixture with `n` lines.
fn long_invoice(n: usize) -> String {
    let start = CII.find("<ram:IncludedSupplyChainTradeLineItem>").expect("a line");
    let end = CII.find("</ram:IncludedSupplyChainTradeLineItem>").expect("its end") + "</ram:IncludedSupplyChainTradeLineItem>".len();
    let line = &CII[start..end];
    let lines: String = (1..=n).map(|i| line.replace("<ram:LineID>1</ram:LineID>", &format!("<ram:LineID>{i}</ram:LineID>"))).collect();
    format!("{}{lines}{}", &CII[..start], &CII[end..])
}

#[test]
fn three_hundred_lines_flow_over_pages_with_the_table_header_and_the_page_header_on_each() {
    let xml = long_invoice(300);
    let invoice = xmlcraft_einvoice::model::read(&xml).expect("an invoice");
    assert_eq!(invoice.lines.len(), 300);
    let t = template::read(template::DE).expect("the template");
    let regular = font::Font::load(font::Face::Regular).expect("font");
    let bold = font::Font::load(font::Face::Bold).expect("font");
    let laid = lay_out(&t, &invoice, &regular, &bold).expect("laid out");
    assert_eq!(laid.pages.len(), 8, "a stable page count");
    let texts = |page: &xmlcraft_invoice_pdf::layout::Page| -> Vec<String> {
        page.items.iter().filter_map(|i| if let Item::Text { text, .. } = i { Some(text.clone()) } else { None }).collect()
    };
    for (i, page) in laid.pages.iter().enumerate() {
        let t = texts(page);
        assert!(t.contains(&"Beispiel Lieferant GmbH".to_owned()), "the header on page {}", i + 1);
        assert!(t.contains(&format!("Seite {} von 8", i + 1)), "the footer on page {}: {t:?}", i + 1);
        if i < 7 {
            assert!(t.contains(&"Bezeichnung".to_owned()), "the table header on page {}", i + 1);
        }
        // Nothing is drawn below the footer's top or over the page edge.
        for item in &page.items {
            if let Item::Text { y, x, .. } = item {
                assert!(*y > 0.0 && *y < laid.size.1 && *x >= 0.0 && *x < laid.size.0, "{item:?}");
            }
        }
    }
    // Every line is drawn once.
    let all: Vec<String> = laid.pages.iter().flat_map(texts).collect();
    for n in [1, 150, 300] {
        assert_eq!(all.iter().filter(|t| **t == n.to_string()).count(), 1, "line {n}");
    }
    let out = render(&xml, xml.as_bytes(), &options(TemplateSource::De)).expect("renders");
    assert_eq!(out.pages, 8);
    let pdf = Pdf::open(out.bytes).expect("reads");
    assert_eq!(metadata_and_pages(&pdf).1.len(), 8);
}

#[test]
fn unknown_placeholders_are_warnings_and_template_errors_are_located() {
    let t = r#"<invoice-template version="1"><body><block>{number} {seller.nmae}</block><block if="buyer.foo">x</block><block>{page}</block></body></invoice-template>"#;
    let out = render(CII, CII.as_bytes(), &options(TemplateSource::Text(t))).expect("renders");
    let w = out.warnings.join("\n");
    assert!(w.contains("{seller.nmae} names nothing"), "{w}");
    assert!(w.contains("if=\"buyer.foo\""), "{w}");
    assert!(w.contains("header and the footer only"), "{w}");
    let e = render(CII, CII.as_bytes(), &options(TemplateSource::Text("<invoice-template version=\"1\">\n<body><x/></body></invoice-template>")))
        .expect_err("refused");
    assert!(matches!(&e, RenderError::Template(t) if t.line == 2), "{e:?}");
    assert!(matches!(render("<a/>", b"<a/>", &options(TemplateSource::De)), Err(RenderError::NotAnInvoice(_))));
}

#[test]
fn characters_the_face_lacks_are_reported_not_dropped() {
    let xml = CII.replace("<ram:Name>Beratung</ram:Name>", "<ram:Name>Beratung 漢字</ram:Name>");
    let out = render(&xml, xml.as_bytes(), &options(TemplateSource::De)).expect("renders");
    assert!(out.warnings.iter().any(|w| w.contains("U+6F22")), "{:?}", out.warnings);
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    fn escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;")
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

        /// Arbitrary (bounded) values and line counts never panic the layout or the writer.
        #[test]
        fn any_invoice_is_laid_out_and_written(
            name in "\\PC{0,300}",
            amount in "[-0-9.,a-z ]{0,30}",
            date in "[0-9a-z-]{0,12}",
            lines in 0usize..40,
            words in proptest::collection::vec("\\PC{0,40}", 0..30),
        ) {
            let xml = long_invoice(lines)
                .replace("Beratung", &escape(&name))
                .replace("<ram:DuePayableAmount>119.00", &format!("<ram:DuePayableAmount>{}", escape(&amount)))
                .replace(">20261101<", &format!(">{}<", escape(&date)))
                .replace("Zahlbar innerhalb von 30 Tagen.", &escape(&words.join(" ")));
            for template in [TemplateSource::De, TemplateSource::En] {
                let out = render(&xml, xml.as_bytes(), &options(template));
                prop_assert!(out.is_ok(), "{:?}", out.err());
            }
        }
    }
}

/// Writes the German rendering of the CII fixture where `XMLCRAFT_INVOICE_PDF_SAMPLE` says (to look at it); nothing otherwise.
#[test]
fn a_sample_is_written_when_asked() {
    let Ok(path) = std::env::var("XMLCRAFT_INVOICE_PDF_SAMPLE") else { return };
    let out = render(CII, CII.as_bytes(), &options(TemplateSource::De)).expect("renders");
    std::fs::write(path, out.bytes).expect("written");
}
