//! `pdf_files`, reading PDFs built here through the root confinement.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn files(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("pdf_files", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

/// A small PDF with one embedded file, a classic cross-reference table.
fn pdf(name: &str, mime: &str, data: &[u8]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let mut object = |out: &mut Vec<u8>, number: u32, body: &[u8]| {
        offsets.push((number, out.len()));
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    object(&mut out, 1, b"<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(a) 2 0 R] >> >> >>");
    object(&mut out, 2, format!("<< /F ({name}) /UF ({name}) /AFRelationship /Data /EF << /F 3 0 R >> >>").as_bytes());
    let mut stream =
        format!("<< /Type /EmbeddedFile /Subtype /{mime} /Params << /Size {} >> /Length {} >>\nstream\n", data.len(), data.len()).into_bytes();
    stream.extend_from_slice(data);
    stream.extend_from_slice(b"\nendstream");
    object(&mut out, 3, &stream);
    let table = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 3\n");
    for (_, offset) in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn the_embedded_files_are_listed_and_one_is_read_as_text() {
    let dir = tempfile::tempdir().expect("dir");
    let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Invoice>Käse</Invoice>";
    std::fs::write(dir.path().join("i.pdf"), pdf("factur-x.xml", "text#2Fxml", xml.as_bytes())).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let r = files(&mut a, json!({ "path": "i.pdf" })).expect("runs");
    assert_eq!((r["count"].as_u64(), r["recovered"].as_bool()), (Some(1), Some(false)), "{r}");
    let f = &r["files"][0];
    assert_eq!(
        (f["name"].as_str(), f["relationship"].as_str(), f["mime"].as_str(), f["size"].as_u64()),
        (Some("factur-x.xml"), Some("Data"), Some("text/xml"), Some(xml.len() as u64))
    );
    for wanted in [json!("factur-x.xml"), json!(0)] {
        let r = files(&mut a, json!({ "path": "i.pdf", "extract": wanted })).expect("runs");
        assert_eq!(r["extracted"]["text"].as_str(), Some(xml), "{r}");
        assert_eq!(r["extracted"]["bytes"].as_u64(), Some(xml.len() as u64));
    }
    let e = files(&mut a, json!({ "path": "i.pdf", "extract": "nope.xml" })).expect_err("no such file");
    assert!(e.to_string().contains("no embedded file"), "{e}");
    assert!(matches!(files(&mut a, json!({ "path": "i.pdf", "extract": 1.5 })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn a_file_that_is_not_text_has_no_text_and_what_is_not_a_pdf_or_outside_the_root_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let outside = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("b.pdf"), pdf("a.bin", "application#2Foctet-stream", &[0xFF, 0xFE, 0x00, 0xD8, 0x00, 0xD8])).expect("writes");
    std::fs::write(dir.path().join("x.pdf"), b"hello").expect("writes");
    std::fs::write(outside.path().join("o.pdf"), pdf("a.xml", "text#2Fxml", b"<a/>")).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let r = files(&mut a, json!({ "path": "b.pdf", "extract": 0 })).expect("runs");
    assert_eq!((r["extracted"]["bytes"].as_u64(), r["extracted"]["textual"].as_bool()), (Some(6), Some(false)), "the extraction ran: {r}");
    assert!(r["extracted"].get("text").is_none(), "{r}");
    let e = files(&mut a, json!({ "path": "x.pdf" })).expect_err("not a pdf");
    assert!(e.to_string().contains("not a PDF"), "{e}");
    let e = files(&mut a, json!({ "path": outside.path().join("o.pdf").to_str().expect("utf-8") })).expect_err("outside");
    assert!(e.to_string().contains("outside the root"), "{e}");
    assert!(matches!(files(&mut a, json!({})), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn text_is_returned_for_text_files_only_and_not_beyond_the_size_a_response_can_carry() {
    let dir = tempfile::tempdir().expect("dir");
    // Bytes that are valid UTF-8 are not thereby text: the media type and the name decide.
    std::fs::write(dir.path().join("zip.pdf"), pdf("a.bin", "application#2Foctet-stream", b"PK\x03\x04 hello")).expect("writes");
    std::fs::write(dir.path().join("img.pdf"), pdf("logo.png", "image#2Fpng", b"plain ascii")).expect("writes");
    std::fs::write(dir.path().join("named.pdf"), pdf("data.xml", "application#2Foctet-stream", b"<a/>")).expect("writes");
    std::fs::write(dir.path().join("typed.pdf"), pdf("noext", "application#2Fvnd.x+xml", b"<a/>")).expect("writes");
    let huge = vec![b'a'; 4 * 1024 * 1024 + 1];
    std::fs::write(dir.path().join("huge.pdf"), pdf("big.xml", "text#2Fxml", &huge)).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let extract = |a: &mut Automation, name: &str| files(a, json!({ "path": name, "extract": 0 })).expect("runs")["extracted"].clone();
    assert_eq!(extract(&mut a, "zip.pdf")["textual"], false);
    assert!(extract(&mut a, "zip.pdf").get("text").is_none());
    assert_eq!(extract(&mut a, "img.pdf")["textual"], false);
    assert_eq!(extract(&mut a, "named.pdf")["text"], "<a/>", "a generic media type and an XML name");
    assert_eq!(extract(&mut a, "typed.pdf")["text"], "<a/>", "an XML media type and no extension");
    let big = extract(&mut a, "huge.pdf");
    assert!(big.get("text").is_none() && big["text_omitted"].as_str().is_some_and(|m| m.contains("larger")), "{big}");
    assert_eq!(big["bytes"].as_u64(), Some(huge.len() as u64));
}

fn replace(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("pdf_replace_file", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

#[test]
fn the_embedded_xml_is_replaced_in_a_copy_and_the_original_is_not_touched() {
    let dir = tempfile::tempdir().expect("dir");
    let original = pdf("factur-x.xml", "text#2Fxml", b"<old/>");
    std::fs::write(dir.path().join("i.pdf"), &original).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let r = replace(&mut a, json!({ "path": "i.pdf", "text": "<new>ä</new>", "output": "o.pdf" })).expect("runs");
    assert_eq!((r["file"].as_str(), r["added"].as_u64().is_some_and(|n| n > 0)), (Some("factur-x.xml"), true), "{r}");
    assert_eq!(std::fs::read(dir.path().join("i.pdf")).expect("reads"), original, "the input is as it was");
    let written = std::fs::read(dir.path().join("o.pdf")).expect("reads");
    assert_eq!(&written[..original.len()], &original[..], "the original bytes are the beginning of the copy");
    let read = files(&mut a, json!({ "path": "o.pdf", "extract": 0 })).expect("runs");
    assert_eq!(read["extracted"]["text"], "<new>ä</new>", "{read}");
    assert_eq!(read["files"][0]["size"].as_u64(), Some("<new>ä</new>".len() as u64), "the declared size is the new one");
    // An existing output is not replaced unless asked; the input may be replaced in place when asked.
    let e = replace(&mut a, json!({ "path": "i.pdf", "text": "<x/>", "output": "o.pdf" })).expect_err("exists");
    assert!(e.to_string().contains("overwrite"), "{e}");
    replace(&mut a, json!({ "path": "i.pdf", "text": "<x/>", "output": "i.pdf", "overwrite": true })).expect("runs");
    assert_eq!(files(&mut a, json!({ "path": "i.pdf", "extract": "factur-x.xml" })).expect("runs")["extracted"]["text"], "<x/>");
}

#[test]
fn what_cannot_be_written_is_refused_and_nothing_is_written() {
    let dir = tempfile::tempdir().expect("dir");
    let outside = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("i.pdf"), pdf("a.xml", "text#2Fxml", b"<old/>")).expect("writes");
    std::fs::write(dir.path().join("x.pdf"), b"hello").expect("writes");
    std::fs::write(outside.path().join("in.pdf"), pdf("a.xml", "text#2Fxml", b"<old/>")).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let refused = |a: &mut Automation, args: Value, why: &str| {
        let e = replace(a, args.clone()).expect_err("refused");
        assert!(e.to_string().contains(why), "{args}: {e}");
    };
    refused(&mut a, json!({ "path": "i.pdf", "text": "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a/>", "output": "o.pdf" }), "ISO-8859-1");
    refused(&mut a, json!({ "path": "i.pdf", "text": "<a/>", "output": "o.pdf", "file": "nope.xml" }), "no such embedded file");
    refused(&mut a, json!({ "path": "x.pdf", "text": "<a/>", "output": "o.pdf" }), "not a PDF");
    refused(&mut a, json!({ "path": outside.path().join("in.pdf").to_str().expect("utf-8"), "text": "<a/>", "output": "o.pdf" }), "outside the root");
    refused(&mut a, json!({ "path": "i.pdf", "text": "<a/>", "output": outside.path().join("o.pdf").to_str().expect("utf-8") }), "outside the root");
    assert!(!dir.path().join("o.pdf").exists() && !outside.path().join("o.pdf").exists(), "nothing was written");
    // UTF-8 declarations are fine, and so is no declaration.
    replace(&mut a, json!({ "path": "i.pdf", "text": "<?xml version='1.0' encoding='utf-8'?><a/>", "output": "o.pdf" })).expect("runs");
    for bad in [
        json!({ "path": "i.pdf", "text": "<a/>" }),
        json!({ "path": "i.pdf", "text": "<a/>", "output": "p.pdf", "overwrite": "yes" }),
        json!({ "path": "i.pdf", "text": "<a/>", "output": "p.pdf", "file": 1.5 }),
    ] {
        assert!(matches!(replace(&mut a, bad.clone()), Err(ToolError::InvalidArgs(_))), "{bad}");
    }
}

#[test]
fn only_a_real_xml_declaration_can_name_the_encoding_and_a_link_is_not_written_through() {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("i.pdf"), pdf("a.xml", "text#2Fxml", b"<old/>")).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    // A stylesheet instruction that mentions an encoding is not a declaration; neither is a comment or an attribute of an element.
    for fine in [
        "<?xml-stylesheet href=\"a.xsl\" encoding=\"ISO-8859-1\"?><a/>",
        "<a note=\"<?xml version='1.0' encoding='latin1'?>\"/>",
        "<?xml version=\"1.0\" standalone=\"yes\"?><a/>",
        "<?xml version='1.0' encoding='UTF-8' standalone='no'?><a/>",
        "<?xmlencoding=\"latin1\"?><a/>",
    ] {
        replace(&mut a, json!({ "path": "i.pdf", "text": fine, "output": "o.pdf", "overwrite": true })).unwrap_or_else(|e| panic!("{fine}: {e}"));
    }
    for refused in ["<?xml version=\"1.0\" encoding=\"windows-1252\"?><a/>", "<?xml\tversion='1.0'\nencoding = 'UTF-16'?><a/>"] {
        let e = replace(&mut a, json!({ "path": "i.pdf", "text": refused, "output": "p.pdf" })).expect_err(refused);
        assert!(e.to_string().contains("declares the encoding"), "{refused}: {e}");
    }
    #[cfg(unix)]
    {
        // A link at the output, even one that leads nowhere (yet) and out of the root, is not written through.
        let outside = tempfile::tempdir().expect("dir");
        std::os::unix::fs::symlink(outside.path().join("target.pdf"), dir.path().join("link.pdf")).expect("links");
        let e = replace(&mut a, json!({ "path": "i.pdf", "text": "<a/>", "output": "link.pdf", "overwrite": true })).expect_err("a link");
        assert!(e.to_string().contains("symbolic link"), "{e}");
        assert!(!outside.path().join("target.pdf").exists(), "nothing was written outside the root");
    }
}
