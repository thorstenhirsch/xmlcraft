//! Changing an embedded file by an incremental update: the original bytes stay, the new file reads back, and what must not be changed
//! is refused.

mod common;

use common::{Builder, XML, compressed_pdf, invoice_pdf, r};
use proptest::prelude::*;
use xmlcraft_pdf::{Object, Pdf, PdfError, md5_hex};

const WHEN: &str = "D:20261105123000Z";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn params_of(pdf: &Pdf, stream: u32) -> (Option<i64>, Option<Vec<u8>>, Option<Vec<u8>>) {
    let Some(Object::Stream(s)) = pdf.get(r(stream)) else { panic!("a stream") };
    let params = pdf.resolve(s.dict.get("Params").expect("params"));
    let d = params.as_dict().expect("a dictionary");
    (
        d.get("Size").and_then(Object::as_int),
        d.get("ModDate").and_then(Object::as_string).map(<[u8]>::to_vec),
        d.get("CheckSum").and_then(Object::as_string).map(<[u8]>::to_vec),
    )
}

#[test]
fn a_classic_pdf_is_updated_by_appending_and_everything_before_stays_byte_for_byte() {
    let original = invoice_pdf("factur-x.xml", XML);
    let pdf = Pdf::open(original.clone()).expect("opens");
    let file = pdf.embedded_files().remove(0);
    let new_xml = "<Invoice>changed ä</Invoice>";
    let updated = pdf.replace_embedded(&file, new_xml.as_bytes(), WHEN).expect("updates");
    assert_eq!(&updated[..original.len()], &original[..], "the original bytes are untouched");
    assert!(updated.len() > original.len());
    let after = Pdf::open(updated.clone()).expect("the updated file opens");
    assert!(!after.recovered());
    let files = after.embedded_files();
    assert_eq!(files.len(), 1);
    assert_eq!(after.embedded_data(&files[0]).expect("data"), new_xml.as_bytes());
    // The parameters are made right: size, date and the MD5 of the data.
    let (size, date, sum) = params_of(&after, 5);
    assert_eq!(size, Some(new_xml.len() as i64));
    assert_eq!(date.as_deref(), Some(WHEN.as_bytes()));
    assert_eq!(sum.map(|s| s.iter().map(|b| format!("{b:02x}")).collect::<String>()), Some(md5_hex(new_xml.as_bytes())));
    assert_eq!(after.trailer().get("Size").and_then(Object::as_int), Some(6), "the size covers every object");
    // The other objects are as they were, and the stream keeps its name, number and other entries.
    for n in 1..=4 {
        assert_eq!(after.get(r(n)), pdf.get(r(n)), "object {n}");
    }
    assert_eq!(
        (files[0].name.as_str(), files[0].mime.as_deref(), files[0].relationship.as_deref()),
        ("factur-x.xml", Some("text/xml"), Some("Data"))
    );
    // The update is a table and a trailer that point back at the old table.
    let tail = text(&updated[original.len()..]);
    assert!(tail.contains("xref\n5 1\n") && tail.contains("/Prev ") && tail.trim_end().ends_with("%%EOF"), "{tail}");
    assert!(updated.starts_with(b"%PDF-1.7"));
    // The older version is still what a reader that stops at the old table sees.
    let older = Pdf::open(original).expect("opens");
    assert_eq!(older.embedded_data(&older.embedded_files()[0]).expect("data"), XML.as_bytes());
}

#[test]
fn a_file_with_a_cross_reference_stream_is_updated_with_one_and_keeps_its_object_streams() {
    let original = compressed_pdf(XML);
    let pdf = Pdf::open(original.clone()).expect("opens");
    let file = pdf.embedded_files().remove(0);
    let updated = pdf.replace_embedded(&file, b"<n/>", WHEN).expect("updates");
    assert_eq!(&updated[..original.len()], &original[..]);
    let after = Pdf::open(updated.clone()).expect("opens");
    assert!(!after.recovered());
    let files = after.embedded_files();
    assert_eq!(
        (files.len(), files[0].name.as_str(), files[0].relationship.as_deref()),
        (1, "z.xml", Some("Source")),
        "the filespec is in an object stream and still found"
    );
    assert_eq!(after.embedded_data(&files[0]).expect("data"), b"<n/>");
    assert!(text(&updated[original.len()..]).contains("/Type /XRef"), "a stream, as the file had");
    assert_eq!(after.get(r(1)), pdf.get(r(1)), "the catalog, in an object stream");
    // The stream was deflated and stays deflated.
    let Some(Object::Stream(s)) = after.get(r(5)) else { panic!("a stream") };
    assert_eq!(s.dict.get("Filter").and_then(Object::as_name), Some("FlateDecode"));
    // The old file has objects 0 to 11; the update's cross-reference stream is object 12, so the next number is 13.
    assert_eq!((pdf.next_object_number(), after.next_object_number()), (12, 13));
}

#[test]
fn updates_can_follow_updates_and_the_file_identifier_changes_in_its_second_half_only() {
    let mut b = Builder::new();
    b.object(1, "<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(a) 2 0 R] >> >> >>");
    b.object(2, "<< /F (a.xml) /EF << /F 3 0 R >> >>");
    b.stream(3, "/Type /EmbeddedFile /Subtype /text#2Fxml", b"<v1/>");
    let first = b.finish(4, "/ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]");
    let one = Pdf::open(first.clone()).expect("opens");
    let second = one.replace_embedded(&one.embedded_files()[0], b"<v2/>", WHEN).expect("updates");
    let two = Pdf::open(second.clone()).expect("opens");
    let third = two.replace_embedded(&two.embedded_files()[0], b"<v3/>", "D:20261106000000Z").expect("updates");
    assert_eq!(&second[..first.len()], &first[..]);
    assert_eq!(&third[..second.len()], &second[..]);
    let three = Pdf::open(third).expect("opens");
    assert_eq!(three.embedded_data(&three.embedded_files()[0]).expect("data"), b"<v3/>");
    let ids = |p: &Pdf| {
        p.trailer().get("ID").and_then(Object::as_array).map(|a| a.iter().filter_map(|o| o.as_string().map(<[u8]>::to_vec)).collect::<Vec<_>>())
    };
    let (id1, id2, id3) = (ids(&one).expect("id"), ids(&two).expect("id"), ids(&three).expect("id"));
    assert_eq!(
        (id1[0].clone(), id2[0].clone(), id3[0].clone()),
        (id1[0].clone(), id1[0].clone(), id1[0].clone()),
        "the first half names the original"
    );
    assert!(id1[1] != id2[1] && id2[1] != id3[1], "the second half is new each time");
    assert_eq!(id2[1].len(), 16);
    // The dates are the dates of the changes.
    let (_, date, _) = params_of(&three, 3);
    assert_eq!(date.as_deref(), Some(&b"D:20261106000000Z"[..]));
}

#[test]
fn documents_that_would_be_broken_by_a_change_are_not_changed() {
    let build = |catalog_extra: &str| {
        let mut b = Builder::new();
        b.object(1, &format!("<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(a) 2 0 R] >> >> {catalog_extra} >>"));
        b.object(2, "<< /F (a.xml) /EF << /F 3 0 R >> >>");
        b.stream(3, "/Subtype /text#2Fxml", b"<a/>");
        b.finish(4, "")
    };
    for (extra, why) in [("/Perms << /DocMDP 9 0 R >>", "certified"), ("/AcroForm << /SigFlags 3 /Fields [] >>", "signatures")] {
        let pdf = Pdf::open(build(extra)).expect("opens");
        let e = pdf.replace_embedded(&pdf.embedded_files()[0], b"<b/>", WHEN).expect_err("refused");
        assert!(matches!(&e, PdfError::Unsupported(m) if m.contains(why)), "{e}");
        assert!(pdf.why_not_editable().is_some());
    }
    // A form that has fields but no signature is fine.
    let pdf = Pdf::open(build("/AcroForm << /SigFlags 0 /Fields [] >>")).expect("opens");
    assert!(pdf.why_not_editable().is_none());
    // A file read by scanning is not changed.
    let mut bytes = invoice_pdf("a.xml", XML);
    let at = bytes.windows(9).rposition(|w| w == b"startxref").expect("startxref");
    bytes.truncate(at);
    bytes.extend_from_slice(b"startxref\n1\n%%EOF\n");
    let pdf = Pdf::open(bytes).expect("opens");
    assert!(pdf.recovered());
    let e = pdf.replace_embedded(&pdf.embedded_files()[0], b"<b/>", WHEN).expect_err("refused");
    assert!(e.to_string().contains("does not repair"), "{e}");
    // A date that is not a PDF date is not written.
    let pdf = Pdf::open(invoice_pdf("a.xml", XML)).expect("opens");
    let file = pdf.embedded_files().remove(0);
    for bad in ["", "2026-11-05", "D:2026)(", "D:\u{e4}"] {
        assert!(pdf.replace_embedded(&file, b"x", bad).is_err(), "{bad:?}");
    }
}

proptest! {
    #[test]
    fn any_data_reads_back_and_the_original_stays(data in proptest::collection::vec(any::<u8>(), 0..300), compressed in proptest::bool::ANY) {
        let original = if compressed { compressed_pdf(XML) } else { invoice_pdf("a.xml", XML) };
        let pdf = Pdf::open(original.clone()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let file = pdf.embedded_files().remove(0);
        let updated = pdf.replace_embedded(&file, &data, WHEN).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(&updated[..original.len()], &original[..]);
        let after = Pdf::open(updated).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert!(!after.recovered());
        prop_assert_eq!(after.embedded_data(&after.embedded_files()[0]).map_err(|e| TestCaseError::fail(e.to_string()))?, data);
    }
}

/// Writes a PDF and its updated version to the directory in `XMLCRAFT_PDF_DUMP`, for checking with other programs (`pdfinfo`,
/// `pdfdetach`, Ghostscript) as black-box oracles. Run with `cargo test -p xmlcraft-pdf --test update -- --ignored`.
#[test]
#[ignore = "writes files for other programs to check"]
fn dump_for_other_programs() {
    let Ok(dir) = std::env::var("XMLCRAFT_PDF_DUMP") else { return };
    let original = invoice_pdf("factur-x.xml", XML);
    let pdf = Pdf::open(original.clone()).expect("opens");
    let updated = pdf.replace_embedded(&pdf.embedded_files()[0], b"<Invoice>changed</Invoice>", WHEN).expect("updates");
    std::fs::write(format!("{dir}/original.pdf"), original).expect("writes");
    std::fs::write(format!("{dir}/updated.pdf"), updated).expect("writes");
}

#[test]
fn a_signature_that_the_flags_do_not_announce_is_found_in_the_fields() {
    let build = |fields: &str| {
        let mut b = Builder::new();
        b.object(1, &format!("<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(a) 2 0 R] >> >> /AcroForm << /Fields [{fields}] >> >>"));
        b.object(2, "<< /F (a.xml) /EF << /F 3 0 R >> >>");
        b.stream(3, "/Subtype /text#2Fxml", b"<a/>");
        b.object(4, "<< /FT /Sig /T (s) /V 5 0 R >>");
        b.object(5, "<< /Type /Sig /Filter /Adobe.PPKLite >>");
        b.object(6, "<< /FT /Sig /T (empty) >>");
        b.object(7, "<< /FT /Tx /Kids [4 0 R] >>");
        b.finish(8, "")
    };
    for (fields, signed) in [("4 0 R", true), ("7 0 R", true), ("6 0 R", false), ("", false)] {
        let pdf = Pdf::open(build(fields)).expect("opens");
        assert_eq!(pdf.why_not_editable().is_some(), signed, "{fields:?}: {:?}", pdf.why_not_editable());
    }
}

#[test]
fn a_replaced_stream_of_a_later_generation_is_recorded_with_it() {
    // The embedded stream is object 3 generation 2, in a file whose cross-reference is a table; the update's table says generation 2 too.
    let mut b = Builder::new();
    b.object(1, "<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(a) 2 0 R] >> >> >>");
    b.object(2, "<< /F (a.xml) /EF << /F 3 2 R >> >>");
    b.offsets.push((3, b.out.len()));
    b.out.extend_from_slice(b"3 2 obj\n<< /Subtype /text#2Fxml /Length 5 >>\nstream\n<old/>\nendstream\nendobj\n");
    let mut bytes = b.finish(4, "");
    // The table is one subsection, `1 3`, of three lines of twenty bytes; the third is object 3. (Searched as bytes: the header of the
    // file has some that are not text.)
    let at = bytes.windows(4).rposition(|w| w == b"1 3\n").expect("the subsection") + 4 + 2 * 20;
    bytes[at + 11..at + 16].copy_from_slice(b"00002");
    let pdf = Pdf::open(bytes).expect("opens");
    assert!(!pdf.recovered(), "the patched table is read");
    let file = pdf.embedded_files().remove(0);
    assert_eq!(file.stream.generation, 2);
    let updated = pdf.replace_embedded(&file, b"<new/>", WHEN).expect("updates");
    let after = Pdf::open(updated.clone()).expect("opens");
    assert_eq!(after.embedded_data(&after.embedded_files()[0]).expect("data"), b"<new/>");
    assert!(text_of(&updated).contains("xref\n3 1\n") && text_of(&updated).contains(" 00002 n "), "{}", text_of(&updated));
}

fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn only_real_pdf_dates_are_written_and_reals_of_the_old_dictionary_are_kept_as_they_were() {
    let pdf = Pdf::open(invoice_pdf("a.xml", XML)).expect("opens");
    let file = pdf.embedded_files().remove(0);
    for good in [
        "D:2026",
        "D:202611",
        "D:20261105",
        "D:2026110512",
        "D:202611051230",
        "D:20261105123000",
        "D:20261105123000Z",
        "D:20261105123000+01'00'",
        "D:20261105123000-05'30",
        "D:20261105123000+01",
        "D:20261105123000Z00'00'",
    ] {
        assert!(pdf.replace_embedded(&file, b"x", good).is_ok(), "{good}");
    }
    for bad in [
        "D:",
        "D:garbage",
        "D:202",
        "D:20261305",
        "D:20261132",
        "D:2026110525",
        "D:202611051260",
        "D:20261105123060",
        "D:20261105123000X",
        "D:20261105123000+25'00'",
        "D:20261105123000+01'60'",
        "D:2026110",
        "d:2026",
        "D:20261105123000Zjunk",
        "D:20261105123000Z0",
    ] {
        assert!(pdf.replace_embedded(&file, b"x", bad).is_err(), "{bad}");
    }
    // A real in the stream's dictionary is written the way it reads back.
    let mut b = Builder::new();
    b.object(1, "<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(a) 2 0 R] >> >> >>");
    b.object(2, "<< /F (a.xml) /EF << /F 3 0 R >> >>");
    b.stream(3, "/Subtype /text#2Fxml /Scale 0.1234567891 /Tiny 0.0000001", b"<a/>");
    let pdf = Pdf::open(b.finish(4, "")).expect("opens");
    let updated = pdf.replace_embedded(&pdf.embedded_files()[0], b"<b/>", WHEN).expect("updates");
    let tail = text_of(&updated[pdf.bytes().len()..]);
    assert!(tail.contains("/Scale 0.1234567891") && tail.contains("/Tiny 0.0000001"), "{tail}");
}

#[test]
fn the_row_of_a_replaced_stream_in_a_cross_reference_stream_has_its_generation() {
    let original = common::compressed_pdf_with(XML, common::Variant::LaterGeneration);
    let pdf = Pdf::open(original.clone()).expect("opens");
    assert!(!pdf.recovered());
    let file = pdf.embedded_files().remove(0);
    assert_eq!(file.stream.generation, 1);
    let updated = pdf.replace_embedded(&file, b"<g/>", WHEN).expect("updates");
    let after = Pdf::open(updated.clone()).expect("opens");
    assert!(!after.recovered(), "the update's own stream is read");
    assert_eq!(after.embedded_data(&after.embedded_files()[0]).expect("data"), b"<g/>", "the reference `5 1 R` finds the new stream");
    assert!(text_of(&updated[original.len()..]).contains("5 1 obj"));
}
