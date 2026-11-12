//! Writing the laid-out invoice as a PDF with PdfCraft's object layer (`pdfcraft-cos`): pages of text and rules, the two embedded
//! faces, the invoice XML as an associated embedded file, and the XMP metadata of Factur-X and PDF/A-3.
//!
//! The structure follows the Factur-X 1.07.2 specification (= ZUGFeRD 2.3), section 6: the XML is embedded at document level, named
//! in the catalog's `/AF` array and in the `/EmbeddedFiles` name tree, with the MIME type `text/xml` and `/Params` with a `/ModDate`
//! (§6.2, §6.2.1); the `/AFRelationship` follows the table of §6.2.2; the XMP carries the Factur-X extension schema of §6.3.1 and
//! its PDF/A description. PDF/A-3 (ISO 19005-3) further needs every font embedded (done), no encryption (none), and an output intent
//! with an ICC profile: XmlCraft ships none, so a file is identified as PDF/A-3b (`pdfaid`) only when the caller gives a profile.

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, SaveOptions, Stream, write_full};

use crate::font::{Face, Font};
use crate::layout::{Item, Laid};

/// The associated file and how the PDF describes it.
pub struct Attachment<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
    pub relationship: &'a str,
    pub description: &'a str,
}

/// The Factur-X identification of the XMP (§6.3.1): the conformance level, and the file name.
pub struct FacturXMeta<'a> {
    pub level: &'a str,
    pub file_name: &'a str,
}

pub struct Meta<'a> {
    pub title: &'a str,
    pub lang: &'a str,
    /// Seconds since 1970 (UTC): the creation and modification time.
    pub time: i64,
    pub facturx: Option<FacturXMeta<'a>>,
    /// The output intent's ICC profile and its number of components; with it the file says it is PDF/A-3b.
    pub icc: Option<(&'a [u8], u8)>,
    /// The file identifier (16 bytes).
    pub id: [u8; 16],
}

fn dict(pairs: Vec<(&str, Object)>) -> Dict {
    let mut d = Dict::new();
    for (k, v) in pairs {
        d.set(k.as_bytes().to_vec(), v);
    }
    d
}

fn name(n: &str) -> Object {
    Object::name(n)
}

fn text(s: &str) -> Object {
    Object::String(PdfString::text(s))
}

fn real(v: f64) -> Object {
    // Two decimals are well below a printer's dot; a number that is not finite is never written.
    let v = if v.is_finite() { (v * 100.0).round() / 100.0 } else { 0.0 };
    if v.fract() == 0.0 && v.abs() < 1e15 { Object::Int(v as i64) } else { Object::Real(v) }
}

fn num(v: f64) -> String {
    let v = if v.is_finite() { (v * 100.0).round() / 100.0 } else { 0.0 };
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn xml_escape(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .map(|c| match c {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            c => c.to_string(),
        })
        .collect()
}

/// The XMP packet (ISO 16684-1) with Dublin Core, XMP basic, PDF, and when they apply PDF/A identification and the Factur-X
/// extension schema with its PDF/A description (ISO 19005-3 §6.6.2.3; Factur-X 1.07.2 §6.3.1).
pub fn xmp(meta: &Meta<'_>) -> String {
    let date = chrono::DateTime::from_timestamp(meta.time, 0).unwrap_or_default().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut s = String::from(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n",
    );
    if meta.icc.is_some() {
        s.push_str("<rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\">\n<pdfaid:part>3</pdfaid:part>\n<pdfaid:conformance>B</pdfaid:conformance>\n</rdf:Description>\n");
    }
    s.push_str(&format!(
        "<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n<dc:format>application/pdf</dc:format>\n<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>\n</rdf:Description>\n",
        xml_escape(meta.title)
    ));
    s.push_str(&format!(
        "<rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">\n<xmp:CreateDate>{date}</xmp:CreateDate>\n<xmp:ModifyDate>{date}</xmp:ModifyDate>\n<xmp:MetadataDate>{date}</xmp:MetadataDate>\n<xmp:CreatorTool>XmlCraft</xmp:CreatorTool>\n</rdf:Description>\n"
    ));
    s.push_str(
        "<rdf:Description rdf:about=\"\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">\n<pdf:Producer>XmlCraft</pdf:Producer>\n</rdf:Description>\n",
    );
    if let Some(fx) = &meta.facturx {
        s.push_str(&format!(
            "<rdf:Description rdf:about=\"\" xmlns:fx=\"urn:factur-x:pdfa:CrossIndustryDocument:invoice:1p0#\">\n<fx:DocumentType>INVOICE</fx:DocumentType>\n<fx:DocumentFileName>{}</fx:DocumentFileName>\n<fx:Version>1.0</fx:Version>\n<fx:ConformanceLevel>{}</fx:ConformanceLevel>\n</rdf:Description>\n",
            xml_escape(fx.file_name),
            xml_escape(fx.level)
        ));
        let property = |name: &str, description: &str| {
            format!(
                "<rdf:li rdf:parseType=\"Resource\">\n<pdfaProperty:name>{name}</pdfaProperty:name>\n<pdfaProperty:valueType>Text</pdfaProperty:valueType>\n<pdfaProperty:category>external</pdfaProperty:category>\n<pdfaProperty:description>{description}</pdfaProperty:description>\n</rdf:li>\n"
            )
        };
        s.push_str(
            "<rdf:Description rdf:about=\"\" xmlns:pdfaExtension=\"http://www.aiim.org/pdfa/ns/extension/\" xmlns:pdfaSchema=\"http://www.aiim.org/pdfa/ns/schema#\" xmlns:pdfaProperty=\"http://www.aiim.org/pdfa/ns/property#\">\n<pdfaExtension:schemas>\n<rdf:Bag>\n<rdf:li rdf:parseType=\"Resource\">\n<pdfaSchema:schema>Factur-X PDFA Extension Schema</pdfaSchema:schema>\n<pdfaSchema:namespaceURI>urn:factur-x:pdfa:CrossIndustryDocument:invoice:1p0#</pdfaSchema:namespaceURI>\n<pdfaSchema:prefix>fx</pdfaSchema:prefix>\n<pdfaSchema:property>\n<rdf:Seq>\n",
        );
        s.push_str(&property("DocumentFileName", "The name of the embedded XML document"));
        s.push_str(&property("DocumentType", "The type of the hybrid document in capital letters, e.g. INVOICE or ORDER"));
        s.push_str(&property("Version", "The actual version of the standard applying to the embedded XML document"));
        s.push_str(&property("ConformanceLevel", "The conformance level of the embedded XML document"));
        s.push_str("</rdf:Seq>\n</pdfaSchema:property>\n</rdf:li>\n</rdf:Bag>\n</pdfaExtension:schemas>\n</rdf:Description>\n");
    }
    s.push_str("</rdf:RDF>\n</x:xmpmeta>\n");
    // Room for an editor to grow the packet in place (ISO 16684-1 §7.3.3).
    for _ in 0..20 {
        s.push_str(&" ".repeat(99));
        s.push('\n');
    }
    s.push_str("<?xpacket end=\"w\"?>");
    s
}

/// The content stream of a page, encoding its text with the faces (which remember the glyphs used).
fn content(items: &[Item], height: f64, regular: &mut Font, bold: &mut Font) -> Vec<u8> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Item::Rule { x1, y1, x2, y2, width, gray } => {
                out.extend_from_slice(
                    format!("{} G {} w {} {} m {} {} l S\n", num(*gray), num(*width), num(*x1), num(height - y1), num(*x2), num(height - y2))
                        .as_bytes(),
                );
            }
            Item::Text { x, y, size, face, gray, text } => {
                let (font, resource) = match face {
                    Face::Regular => (&mut *regular, "F1"),
                    Face::Bold => (&mut *bold, "F2"),
                };
                let glyphs = font.encode(text);
                if glyphs.is_empty() {
                    continue;
                }
                let hex: String = glyphs.iter().map(|b| format!("{b:02X}")).collect();
                out.extend_from_slice(
                    format!("BT {} g /{resource} {} Tf 1 0 0 1 {} {} Tm <{hex}> Tj ET\n", num(*gray), num(*size), num(*x), num(height - y))
                        .as_bytes(),
                );
            }
        }
    }
    out
}

/// The Type 0 font of a face, with its descendant, descriptor, subset program and `/ToUnicode` (ISO 32000-1 §9.7.4–9.7.6).
fn font_object(doc: &mut Document, font: &Font) -> Result<ObjRef, String> {
    let program = font.subset().map_err(|e| e.0)?;
    let length = i64::try_from(program.len()).map_err(|_| "font program too large")?;
    let file = doc.add(Object::Stream(Stream::flate(dict(vec![("Length1", Object::Int(length))]), &program)));
    let base = font.subset_name();
    let descriptor = doc.add(Object::Dict(dict(vec![
        ("Type", name("FontDescriptor")),
        ("FontName", name(&base)),
        ("Flags", Object::Int(32)),
        ("FontBBox", Object::Array(font.bbox.iter().map(|v| real(*v)).collect())),
        ("ItalicAngle", real(font.italic_angle)),
        ("Ascent", real(font.ascent)),
        ("Descent", real(font.descent)),
        ("CapHeight", real(font.cap_height)),
        ("StemV", Object::Int(if font.face == Face::Bold { 120 } else { 80 })),
        ("FontFile2", Object::Ref(file)),
    ])));
    let widths: Vec<Object> = font
        .widths()
        .into_iter()
        .flat_map(|(first, ws)| [Object::Int(i64::from(first)), Object::Array(ws.into_iter().map(real).collect())])
        .collect();
    let cid = doc.add(Object::Dict(dict(vec![
        ("Type", name("Font")),
        ("Subtype", name("CIDFontType2")),
        ("BaseFont", name(&base)),
        (
            "CIDSystemInfo",
            Object::Dict(dict(vec![
                ("Registry", Object::String(PdfString::literal(b"Adobe".to_vec()))),
                ("Ordering", Object::String(PdfString::literal(b"Identity".to_vec()))),
                ("Supplement", Object::Int(0)),
            ])),
        ),
        ("FontDescriptor", Object::Ref(descriptor)),
        ("W", Object::Array(widths)),
        ("CIDToGIDMap", name("Identity")),
    ])));
    let to_unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), &font.to_unicode())));
    Ok(doc.add(Object::Dict(dict(vec![
        ("Type", name("Font")),
        ("Subtype", name("Type0")),
        ("BaseFont", name(&base)),
        ("Encoding", name("Identity-H")),
        ("DescendantFonts", Object::Array(vec![Object::Ref(cid)])),
        ("ToUnicode", Object::Ref(to_unicode)),
    ]))))
}

/// The PDF of a laid-out invoice with its attachment.
pub fn write(laid: &Laid, attachment: &Attachment<'_>, meta: &Meta<'_>) -> Result<Vec<u8>, String> {
    let mut regular = Font::load(Face::Regular).map_err(|e| e.0)?;
    let mut bold = Font::load(Face::Bold).map_err(|e| e.0)?;
    let mut doc = Document::new_empty();
    doc.set_version("1.7");
    let (w, h) = laid.size;
    let contents: Vec<Vec<u8>> = laid.pages.iter().map(|p| content(&p.items, h, &mut regular, &mut bold)).collect();
    let f1 = font_object(&mut doc, &regular)?;
    let f2 = font_object(&mut doc, &bold)?;
    let resources = dict(vec![("Font", Object::Dict(dict(vec![("F1", Object::Ref(f1)), ("F2", Object::Ref(f2))])))]);
    // The page tree: its object number first, so the pages can name their parent.
    let pages_ref = doc.add(Object::Null);
    let mut kids = Vec::new();
    for c in contents {
        let stream = doc.add(Object::Stream(Stream::flate(Dict::new(), &c)));
        kids.push(Object::Ref(doc.add(Object::Dict(dict(vec![
            ("Type", name("Page")),
            ("Parent", Object::Ref(pages_ref)),
            ("MediaBox", Object::Array(vec![Object::Int(0), Object::Int(0), real(w), real(h)])),
            ("Resources", Object::Dict(resources.clone())),
            ("Contents", Object::Ref(stream)),
        ])))));
    }
    let count = i64::try_from(kids.len()).map_err(|_| "too many pages")?;
    doc.set(pages_ref, Object::Dict(dict(vec![("Type", name("Pages")), ("Kids", Object::Array(kids)), ("Count", Object::Int(count))])));
    // The embedded file (Factur-X §6.2: text/xml, /Params with /ModDate, the name tree and /AF).
    let date = xmlcraft_pdf::pdf_date(meta.time);
    let size = i64::try_from(attachment.data.len()).map_err(|_| "the XML is too large")?;
    let checksum: Vec<u8> =
        (0..16).filter_map(|i| xmlcraft_pdf::md5_hex(attachment.data).get(i * 2..i * 2 + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect();
    let embedded = doc.add(Object::Stream(Stream::flate(
        dict(vec![
            ("Type", name("EmbeddedFile")),
            ("Subtype", name("text/xml")),
            (
                "Params",
                Object::Dict(dict(vec![
                    ("ModDate", Object::String(PdfString::literal(date.as_bytes().to_vec()))),
                    ("CreationDate", Object::String(PdfString::literal(date.as_bytes().to_vec()))),
                    ("Size", Object::Int(size)),
                    ("CheckSum", Object::String(PdfString { bytes: checksum, hex: true })),
                ])),
            ),
        ]),
        attachment.data,
    )));
    let filespec = doc.add(Object::Dict(dict(vec![
        ("Type", name("Filespec")),
        ("F", Object::String(PdfString::literal(attachment.name.as_bytes().to_vec()))),
        ("UF", text(attachment.name)),
        ("Desc", text(attachment.description)),
        ("AFRelationship", name(attachment.relationship)),
        ("EF", Object::Dict(dict(vec![("F", Object::Ref(embedded)), ("UF", Object::Ref(embedded))]))),
    ])));
    let metadata =
        doc.add(Object::Stream(Stream::from_raw(dict(vec![("Type", name("Metadata")), ("Subtype", name("XML"))]), xmp(meta).into_bytes())));
    let mut catalog = vec![
        ("Type", name("Catalog")),
        ("Pages", Object::Ref(pages_ref)),
        ("Metadata", Object::Ref(metadata)),
        ("Lang", text(meta.lang)),
        ("AF", Object::Array(vec![Object::Ref(filespec)])),
        (
            "Names",
            Object::Dict(dict(vec![(
                "EmbeddedFiles",
                Object::Dict(dict(vec![(
                    "Names",
                    Object::Array(vec![Object::String(PdfString::literal(attachment.name.as_bytes().to_vec())), Object::Ref(filespec)]),
                )])),
            )])),
        ),
        ("ViewerPreferences", Object::Dict(dict(vec![("DisplayDocTitle", Object::Bool(true))]))),
        ("PageMode", name("UseAttachments")),
    ];
    if let Some((profile, components)) = meta.icc {
        let icc = doc.add(Object::Stream(Stream::flate(dict(vec![("N", Object::Int(i64::from(components)))]), profile)));
        let intent = dict(vec![
            ("Type", name("OutputIntent")),
            ("S", name("GTS_PDFA1")),
            ("OutputConditionIdentifier", text("Custom")),
            ("Info", text("The ICC profile given to XmlCraft")),
            ("DestOutputProfile", Object::Ref(icc)),
        ]);
        catalog.push(("OutputIntents", Object::Array(vec![Object::Dict(intent)])));
    }
    let root = match doc.trailer().get(b"Root") {
        Some(Object::Ref(r)) => *r,
        _ => doc.add(Object::Null),
    };
    doc.set(root, Object::Dict(dict(catalog)));
    let id = Object::String(PdfString { bytes: meta.id.to_vec(), hex: true });
    doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(root));
    doc.trailer_mut().set(b"ID".to_vec(), Object::Array(vec![id.clone(), id]));
    write_full(&doc, &SaveOptions { object_streams: true, ..SaveOptions::default() }).map_err(|e| format!("the PDF could not be written: {e}"))
}

/// What an ICC profile is, from its header (ICC.1:2010 §7.2): the number of components of an output (`prtr`) or display (`mntr`)
/// profile of version 2 or 4, which a PDF/A output intent may use (ISO 19005-2 §6.2.3).
pub fn icc_components(profile: &[u8]) -> Result<u8, String> {
    let get = |r: std::ops::Range<usize>| profile.get(r).ok_or_else(|| "the ICC profile is shorter than its header".to_owned());
    if profile.len() < 128 {
        return Err("the ICC profile is shorter than its header".into());
    }
    let declared = get(0..4)
        .and_then(|b| <[u8; 4]>::try_from(b).map_err(|_| "the ICC profile is shorter than its header".to_owned()))
        .map(u32::from_be_bytes)?;
    if usize::try_from(declared).ok() != Some(profile.len()) {
        return Err("the ICC profile's size does not match its header".into());
    }
    if get(36..40)? != b"acsp" {
        return Err("this is no ICC profile (no acsp signature)".into());
    }
    let version = *get(8..9)?.first().unwrap_or(&0);
    if !(2..=4).contains(&version) {
        return Err(format!("ICC version {version} is not one PDF/A-3 allows (2 to 4)"));
    }
    match get(12..16)? {
        b"prtr" | b"mntr" => {}
        other => return Err(format!("a PDF/A output intent needs an output or display profile, not class {}", String::from_utf8_lossy(other))),
    }
    match get(16..20)? {
        b"RGB " => Ok(3),
        b"GRAY" => Ok(1),
        b"CMYK" => Ok(4),
        other => Err(format!("the ICC profile's colour space {} is not RGB, gray or CMYK", String::from_utf8_lossy(other))),
    }
}
