//! Reading PDFs built here by hand: classic cross-references, cross-reference streams with object streams and predictors, incremental
//! updates, damaged files, and the files they embed.

use proptest::prelude::*;
use xmlcraft_pdf::{ObjRef, Object, Pdf, PdfError, text_string};

const XML: &str = "<?xml version=\"1.0\"?><rsm:CrossIndustryInvoice xmlns:rsm=\"urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100\"/>";

fn r(n: u32) -> ObjRef {
    ObjRef { number: n, generation: 0 }
}

/// A PDF made of objects written one after the other, with a classic cross-reference table.
struct Builder {
    out: Vec<u8>,
    offsets: Vec<(u32, usize)>,
}

impl Builder {
    fn new() -> Self {
        Self { out: b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec(), offsets: Vec::new() }
    }

    fn object(&mut self, number: u32, body: &str) {
        self.offsets.push((number, self.out.len()));
        self.out.extend_from_slice(format!("{number} 0 obj\n{body}\nendobj\n").as_bytes());
    }

    fn stream(&mut self, number: u32, dict: &str, data: &[u8]) {
        self.offsets.push((number, self.out.len()));
        self.out.extend_from_slice(format!("{number} 0 obj\n<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes());
        self.out.extend_from_slice(data);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
    }

    /// The table for the objects written since `from`, the trailer, and startxref.
    fn finish(mut self, size: u32, trailer_extra: &str) -> Vec<u8> {
        let table = self.out.len();
        self.out.extend_from_slice(b"xref\n");
        let mut sorted = self.offsets.clone();
        sorted.sort_unstable();
        // One subsection for object 0 and one for each run of numbers.
        self.out.extend_from_slice(b"0 1\n0000000000 65535 f \n");
        let mut i = 0;
        while i < sorted.len() {
            let mut j = i;
            while j + 1 < sorted.len() && sorted[j + 1].0 == sorted[j].0 + 1 {
                j += 1;
            }
            self.out.extend_from_slice(format!("{} {}\n", sorted[i].0, j - i + 1).as_bytes());
            for (_, offset) in &sorted[i..=j] {
                self.out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
            }
            i = j + 1;
        }
        self.out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R {trailer_extra} >>\nstartxref\n{table}\n%%EOF\n").as_bytes());
        self.out
    }
}

fn invoice_pdf(name: &str, xml: &str) -> Vec<u8> {
    let mut b = Builder::new();
    b.object(1, "<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(invoice.xml) 4 0 R] >> >> /AF [4 0 R] >>");
    b.object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    b.object(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>");
    b.object(4, &format!("<< /Type /Filespec /F ({name}) /UF ({name}) /AFRelationship /Data /Desc (The invoice) /EF << /F 5 0 R /UF 5 0 R >> >>"));
    b.stream(5, &format!("/Type /EmbeddedFile /Subtype /text#2Fxml /Params << /Size {} >>", xml.len()), xml.as_bytes());
    b.finish(6, "")
}

#[test]
fn a_classic_pdf_lists_and_gives_its_embedded_file_once() {
    let pdf = Pdf::open(invoice_pdf("factur-x.xml", XML)).expect("opens");
    assert!(!pdf.recovered());
    let files = pdf.embedded_files();
    assert_eq!(files.len(), 1, "in the name tree and in /AF, but one file: {files:?}");
    let file = &files[0];
    assert_eq!(
        (file.name.as_str(), file.description.as_deref(), file.relationship.as_deref(), file.mime.as_deref(), file.size),
        ("factur-x.xml", Some("The invoice"), Some("Data"), Some("text/xml"), Some(XML.len() as u64))
    );
    assert_eq!((file.file_spec, file.stream), (Some(r(4)), r(5)));
    assert_eq!(pdf.embedded_data(file).expect("data"), XML.as_bytes());
    assert_eq!(pdf.next_object_number(), 6);
}

#[test]
fn objects_are_read_with_their_types() {
    let pdf = Pdf::open(invoice_pdf("a.xml", XML)).expect("opens");
    let catalog = pdf.get(r(1)).expect("catalog");
    assert_eq!(catalog.as_dict().and_then(|d| d.get("Type")).and_then(Object::as_name), Some("Catalog"));
    assert_eq!(pdf.get(r(99)), None);
    assert_eq!(pdf.resolve(&Object::Ref(r(99))), Object::Null);
    let pages = pdf.get(r(2)).expect("pages");
    assert_eq!(pages.as_dict().and_then(|d| d.get("Count")).and_then(Object::as_int), Some(1));
}

#[test]
fn text_strings_are_utf16_utf8_or_latin1() {
    assert_eq!(text_string(&[0xFE, 0xFF, 0x00, b'a', 0x20, 0xAC]), "a€");
    assert_eq!(text_string(&[0xFF, 0xFE, b'a', 0x00]), "a");
    assert_eq!(text_string(&[0xEF, 0xBB, 0xBF, b'a', 0xC3, 0xA4]), "aä");
    assert_eq!(text_string(b"plain \xE4"), "plain ä");
    let mut b = Builder::new();
    b.object(1, "<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(x) 2 0 R] >> >> >>");
    b.object(2, "<< /F <FEFF00720065006300680075006E00670020D83DDE00> /EF << /F 3 0 R >> >>");
    b.stream(3, "/Subtype /text#2Fxml", b"<a/>");
    let pdf = Pdf::open(b.finish(4, "")).expect("opens");
    assert_eq!(pdf.embedded_files()[0].name, "rechung 😀");
}

#[test]
fn an_incremental_update_replaces_what_it_names_and_keeps_the_rest() {
    let base = invoice_pdf("a.xml", "<old/>");
    let mut updated = base.clone();
    let start = updated.len();
    // A new version of the stream (object 5) and a table for it that points back at the old one.
    let offset = updated.len();
    updated.extend_from_slice(
        b"5 0 obj\n<< /Type /EmbeddedFile /Subtype /text#2Fxml /Params << /Size 6 >> /Length 6 >>\nstream\n<new/>\nendstream\nendobj\n",
    );
    let table = updated.len();
    let old_table = {
        let text = String::from_utf8_lossy(&base).into_owned();
        let at = text.rfind("startxref\n").expect("startxref") + "startxref\n".len();
        text[at..].lines().next().expect("offset").trim().parse::<usize>().expect("number")
    };
    updated.extend_from_slice(
        format!("xref\n5 1\n{offset:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R /Prev {old_table} >>\nstartxref\n{table}\n%%EOF\n").as_bytes(),
    );
    assert_eq!(&updated[..start], &base[..], "an update only appends");
    let pdf = Pdf::open(updated).expect("opens");
    let files = pdf.embedded_files();
    assert_eq!(pdf.embedded_data(&files[0]).expect("data"), b"<new/>");
    assert!(pdf.get(r(3)).is_some(), "an object the update did not mention is still there");
    // The older version is not read.
    let older = Pdf::open(base).expect("opens");
    assert_eq!(older.embedded_data(&older.embedded_files()[0]).expect("data"), b"<old/>");
}

/// A PDF 1.5 file: the catalog and the filespec are in an object stream, the cross-reference is a stream with a PNG predictor, and the
/// embedded file is deflated.
fn compressed_pdf(xml: &str) -> Vec<u8> {
    use miniz_oxide::deflate::compress_to_vec_zlib;
    let mut out = b"%PDF-1.7\n".to_vec();
    // The object stream (object 10) holds objects 1 (catalog) and 4 (filespec).
    let one = "<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(i) 4 0 R] >> >> >>";
    let four = "<< /F (z.xml) /EF << /UF 5 0 R >> /AFRelationship /Source >>";
    let header = format!("1 0 4 {} ", one.len() + 1);
    let body = format!("{header}{one} {four}");
    let packed = compress_to_vec_zlib(body.as_bytes(), 6);
    let objstm_at = out.len();
    out.extend_from_slice(
        format!("10 0 obj\n<< /Type /ObjStm /N 2 /First {} /Filter /FlateDecode /Length {} >>\nstream\n", header.len(), packed.len()).as_bytes(),
    );
    out.extend_from_slice(&packed);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    let file = compress_to_vec_zlib(xml.as_bytes(), 6);
    let stream_at = out.len();
    out.extend_from_slice(
        format!("5 0 obj\n<< /Type /EmbeddedFile /Subtype /application#2Fxml /Filter /FlateDecode /Length {} >>\nstream\n", file.len()).as_bytes(),
    );
    out.extend_from_slice(&file);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    // The cross-reference stream (object 11): entries 0..=11, W [1 4 1], PNG "Up" predictor (12), 6 bytes a row.
    let table_at = out.len();
    let mut rows: Vec<[u8; 6]> = Vec::new();
    for n in 0..12u32 {
        let (kind, f1, f2): (u8, u32, u8) = match n {
            0 => (0, 0, 255),
            1 => (2, 10, 0),
            4 => (2, 10, 7), // (the index is wrong: the number decides)
            5 => (1, u32::try_from(stream_at).expect("fits"), 0),
            10 => (1, u32::try_from(objstm_at).expect("fits"), 0),
            11 => (1, u32::try_from(table_at).expect("fits"), 0),
            _ => (0, 0, 0),
        };
        let b = f1.to_be_bytes();
        rows.push([kind, b[0], b[1], b[2], b[3], f2]);
    }
    let mut predicted: Vec<u8> = Vec::new();
    let mut previous = [0u8; 6];
    for row in &rows {
        predicted.push(2); // Up
        for i in 0..6 {
            predicted.push(row[i].wrapping_sub(previous[i]));
        }
        previous = *row;
    }
    let packed = compress_to_vec_zlib(&predicted, 6);
    out.extend_from_slice(
        format!(
            "11 0 obj\n<< /Type /XRef /Size 12 /W [1 4 1] /Root 1 0 R /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 6 >> /Length {} >>\nstream\n",
            packed.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&packed);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{table_at}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn cross_reference_streams_object_streams_filters_and_predictors_are_read() {
    let pdf = Pdf::open(compressed_pdf(XML)).expect("opens");
    assert!(!pdf.recovered());
    let files = pdf.embedded_files();
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(
        (files[0].name.as_str(), files[0].relationship.as_deref(), files[0].mime.as_deref()),
        ("z.xml", Some("Source"), Some("application/xml"))
    );
    assert_eq!(pdf.embedded_data(&files[0]).expect("data"), XML.as_bytes());
    assert_eq!(pdf.offset_of(1), None, "the catalog is in an object stream");
    assert!(pdf.offset_of(5).is_some());
}

#[test]
fn a_damaged_cross_reference_is_recovered_by_scanning_and_says_so() {
    let mut bytes = invoice_pdf("a.xml", XML);
    // Wrong startxref, no table.
    let at = bytes.windows(9).rposition(|w| w == b"startxref").expect("startxref");
    bytes.truncate(at);
    bytes.extend_from_slice(b"startxref\n999999\n%%EOF\n");
    let pdf = Pdf::open(bytes).expect("opens");
    assert!(pdf.recovered());
    assert_eq!(pdf.embedded_data(&pdf.embedded_files()[0]).expect("data"), XML.as_bytes());
    // A file with the table gone and the trailer too: the catalog is found by its type.
    let mut bytes = invoice_pdf("a.xml", XML);
    let at = bytes.windows(4).rposition(|w| w == b"xref").expect("xref");
    bytes.truncate(at);
    let pdf = Pdf::open(bytes).expect("opens");
    assert!(pdf.recovered());
    assert_eq!(pdf.embedded_files().len(), 1);
    // The compressed file, with its object stream's entries found by scanning.
    let mut bytes = compressed_pdf(XML);
    let at = bytes.windows(9).rposition(|w| w == b"startxref").expect("startxref");
    bytes.truncate(at);
    bytes.extend_from_slice(b"startxref\n7\n%%EOF\n");
    let pdf = Pdf::open(bytes).expect("opens");
    assert!(pdf.recovered());
    assert_eq!(pdf.embedded_data(&pdf.embedded_files()[0]).expect("data"), XML.as_bytes());
}

#[test]
fn encrypted_and_foreign_files_are_refused() {
    let mut b = Builder::new();
    b.object(1, "<< /Type /Catalog >>");
    b.object(2, "<< /Filter /Standard /V 1 >>");
    let encrypted = b.finish(3, "/Encrypt 2 0 R");
    assert_eq!(Pdf::open(encrypted).err(), Some(PdfError::Encrypted));
    assert_eq!(Pdf::open(b"not a pdf at all".to_vec()).err(), Some(PdfError::NotAPdf));
    assert_eq!(Pdf::open(Vec::new()).err(), Some(PdfError::NotAPdf));
    assert!(matches!(Pdf::open(b"%PDF-1.4\n".to_vec()).err(), Some(PdfError::Corrupt(_))), "no objects, no catalog");
}

#[test]
fn a_stream_with_an_indirect_length_and_a_wrong_length_is_still_read() {
    let mut b = Builder::new();
    b.object(1, "<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(a) 2 0 R] >> >> >>");
    b.object(2, "<< /F (a.xml) /EF << /F 3 0 R /UF 4 0 R >> >>");
    // The length is object 5; the data is "<a/>".
    b.offsets.push((3, b.out.len()));
    b.out.extend_from_slice(b"3 0 obj\n<< /Subtype /text#2Fxml /Length 5 0 R >>\nstream\n<a/>\nendstream\nendobj\n");
    // A length that is wrong (too long): `endstream` decides.
    b.offsets.push((4, b.out.len()));
    b.out.extend_from_slice(b"4 0 obj\n<< /Subtype /text#2Fxml /Length 9999 >>\nstream\n<b/>\nendstream\nendobj\n");
    b.object(5, "4");
    let pdf = Pdf::open(b.finish(6, "")).expect("opens");
    let spec = pdf.get(r(2)).expect("spec");
    let _ = spec;
    let three = pdf.get(r(3)).expect("stream three");
    let Object::Stream(s) = three else { panic!("a stream") };
    assert_eq!(pdf.stream_data(&s).expect("data"), b"<a/>");
    let Object::Stream(s) = pdf.get(r(4)).expect("stream four") else { panic!("a stream") };
    assert_eq!(pdf.stream_data(&s).expect("data"), b"<b/>");
}

#[test]
fn the_syntax_of_strings_names_and_numbers_is_read() {
    let mut b = Builder::new();
    b.object(1, "<< /Type /Catalog /S (a\\(b\\)\\n\\101\\\n c (nested)) /H <48656C6C6F2> /N /A#20B /R -1.5 /I +17 /B true /Z null /A [1 2 R 3 /X] %comment\n /Q 5 >>");
    let pdf = Pdf::open(b.finish(2, "")).expect("opens");
    let catalog = pdf.get(r(1)).expect("catalog");
    let d = catalog.as_dict().expect("dict");
    assert_eq!(d.get("S").and_then(Object::as_string), Some(&b"a(b)\nA c (nested)"[..]));
    assert_eq!(d.get("H").and_then(Object::as_string), Some(&b"Hello "[..]));
    assert_eq!(d.get("N").and_then(Object::as_name), Some("A B"));
    assert_eq!(
        (d.get("R"), d.get("I"), d.get("B"), d.get("Z")),
        (Some(&Object::Real(-1.5)), Some(&Object::Int(17)), Some(&Object::Bool(true)), Some(&Object::Null))
    );
    assert_eq!(
        d.get("A").and_then(Object::as_array),
        Some(&[Object::Ref(ObjRef { number: 1, generation: 2 }), Object::Int(3), Object::Name("X".to_owned())][..]),
        "a reference, 3 and a name"
    );
    assert_eq!(d.get("Q"), Some(&Object::Int(5)), "after a comment");
}

proptest! {
    #[test]
    fn no_bytes_panic_the_reader(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let _ = Pdf::open(bytes);
    }

    #[test]
    fn a_pdf_with_any_bytes_changed_is_read_or_refused_without_a_panic(at in 0usize..400, byte in any::<u8>(), cut in 0usize..400) {
        let mut bytes = invoice_pdf("a.xml", XML);
        let n = bytes.len();
        bytes[at % n] = byte;
        bytes.truncate(n - cut % (n / 2));
        if let Ok(pdf) = Pdf::open(bytes) {
            for f in pdf.embedded_files() {
                let _ = pdf.embedded_data(&f);
            }
        }
    }
}
