//! Reading PDFs built here by hand: classic cross-references, cross-reference streams with object streams and predictors, incremental
//! updates, damaged files, and the files they embed.

use proptest::prelude::*;
use xmlcraft_pdf::{ObjRef, Object, Pdf, PdfError, text_string};

mod common;
use common::{Builder, XML, compressed_pdf, invoice_pdf, r};

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
    // The table starts a line; the last `xref` in the file is the one inside `startxref`.
    let at = bytes.windows(5).rposition(|w| w == b"\nxref").expect("xref") + 1;
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

#[test]
fn an_object_stream_that_a_cross_reference_puts_inside_a_stream_is_not_followed_forever() {
    // The entry for the object stream says it is inside object stream 10, which is itself: reading must end, and ends in a recovery.
    let pdf = Pdf::open(common::compressed_pdf_with(XML, common::Variant::NestedObjectStream)).expect("opens");
    assert!(pdf.recovered(), "the table led nowhere");
    assert_eq!(pdf.embedded_data(&pdf.embedded_files()[0]).expect("data"), XML.as_bytes());
}

#[test]
fn a_cross_reference_stream_with_fewer_rows_than_it_says_is_damaged() {
    let pdf = Pdf::open(common::compressed_pdf_with(XML, common::Variant::MissingRows)).expect("opens");
    assert!(pdf.recovered(), "the table is not trusted, the objects are found by scanning");
    assert_eq!(pdf.embedded_files().len(), 1);
}

#[test]
fn a_hybrid_file_takes_the_objects_of_its_streams_from_the_stream_its_table_names() {
    use miniz_oxide::deflate::compress_to_vec_zlib;
    // Object 4 (the filespec) is in object stream 10; the classic table lists it as free and names a cross-reference stream (11) that
    // says where it is.
    let mut out = b"%PDF-1.5\n".to_vec();
    let mut offsets: Vec<(u32, usize)> = Vec::new();
    let mut add = |out: &mut Vec<u8>, n: u32, body: &[u8]| {
        offsets.push((n, out.len()));
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    add(&mut out, 1, b"<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(i) 4 0 R] >> >> >>");
    let four = "<< /F (h.xml) /EF << /F 5 0 R >> >>";
    let body = format!("4 0 {four}");
    let packed = compress_to_vec_zlib(body.as_bytes(), 6);
    let mut objstm = format!("<< /Type /ObjStm /N 1 /First 4 /Filter /FlateDecode /Length {} >>\nstream\n", packed.len()).into_bytes();
    objstm.extend_from_slice(&packed);
    objstm.extend_from_slice(b"\nendstream");
    add(&mut out, 10, &objstm);
    let mut stream = b"<< /Subtype /text#2Fxml /Length 4 >>\nstream\n<h/>\nendstream".to_vec();
    stream.truncate(stream.len());
    add(&mut out, 5, &stream);
    // The cross-reference stream (11): one row, for object 4: type 2, stream 10, index 0 (W [1 2 1]).
    let rows = [2u8, 0, 10, 0];
    let mut x = format!("<< /Type /XRef /Size 12 /W [1 2 1] /Index [4 1] /Length {} >>\nstream\n", rows.len()).into_bytes();
    x.extend_from_slice(&rows);
    x.extend_from_slice(b"\nendstream");
    add(&mut out, 11, &x);
    let table = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 1\n");
    let at = |n: u32| offsets.iter().find(|(m, _)| *m == n).map_or(0, |(_, o)| *o);
    out.extend_from_slice(format!("{:010} 00000 n \n", at(1)).as_bytes());
    out.extend_from_slice(b"4 1\n0000000000 00000 f \n");
    out.extend_from_slice(format!("5 1\n{:010} 00000 n \n10 2\n{:010} 00000 n \n{:010} 00000 n \n", at(5), at(10), at(11)).as_bytes());
    out.extend_from_slice(format!("trailer\n<< /Size 12 /Root 1 0 R /XRefStm {} >>\nstartxref\n{table}\n%%EOF\n", at(11)).as_bytes());
    let pdf = Pdf::open(out).expect("opens");
    assert!(!pdf.recovered());
    let files = pdf.embedded_files();
    assert_eq!(files.len(), 1, "the filespec is in an object stream that only the hybrid part knows");
    assert_eq!(files[0].name, "h.xml");
}

#[test]
fn an_embedded_file_entry_that_is_not_a_stream_is_not_a_file() {
    let mut b = Builder::new();
    b.object(1, "<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(a) 2 0 R (b) 4 0 R] >> >> >>");
    b.object(2, "<< /F (a.xml) /EF << /F 3 0 R >> >>");
    b.object(3, "<< /Subtype /text#2Fxml >>");
    b.object(4, "<< /F (b.xml) /EF << /F 5 0 R >> >>");
    b.stream(5, "/Subtype /text#2Fxml", b"<b/>");
    let pdf = Pdf::open(b.finish(6, "")).expect("opens");
    let names: Vec<String> = pdf.embedded_files().into_iter().map(|f| f.name).collect();
    assert_eq!(names, ["b.xml"], "a dictionary has no data");
}
