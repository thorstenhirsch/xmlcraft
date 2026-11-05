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
    assert!(r["extracted"].get("text").is_none(), "{r}");
    let e = files(&mut a, json!({ "path": "x.pdf" })).expect_err("not a pdf");
    assert!(e.to_string().contains("not a PDF"), "{e}");
    let e = files(&mut a, json!({ "path": outside.path().join("o.pdf").to_str().expect("utf-8") })).expect_err("outside");
    assert!(e.to_string().contains("outside the root"), "{e}");
    assert!(matches!(files(&mut a, json!({})), Err(ToolError::InvalidArgs(_))));
}
