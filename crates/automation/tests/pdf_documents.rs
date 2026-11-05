//! A PDF in the tools that read and write documents: it is the XML embedded in it, and saving it appends an incremental update.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content};

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    let content = a.call(tool, &args).unwrap_or_else(|e| panic!("{tool}: {e}"));
    match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    }
}

/// A small PDF with one embedded file, a classic cross-reference table.
fn pdf(name: &str, data: &[u8]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let mut object = |out: &mut Vec<u8>, number: u32, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    object(&mut out, 1, b"<< /Type /Catalog /Names << /EmbeddedFiles << /Names [(a) 2 0 R] >> >> >>");
    object(&mut out, 2, format!("<< /F ({name}) /UF ({name}) /AFRelationship /Data /EF << /F 3 0 R >> >>").as_bytes());
    let mut stream =
        format!("<< /Type /EmbeddedFile /Subtype /text#2Fxml /Params << /Size {} >> /Length {} >>\nstream\n", data.len(), data.len()).into_bytes();
    stream.extend_from_slice(data);
    stream.extend_from_slice(b"\nendstream");
    object(&mut out, 3, &stream);
    let table = out.len();
    out.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \n1 3\n");
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n").as_bytes());
    out
}

const INVOICE: &str = "<Invoice><ID>RE-1</ID></Invoice>";

#[test]
fn a_pdf_given_by_path_is_read_as_the_xml_embedded_in_it() {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("i.pdf"), pdf("factur-x.xml", INVOICE.as_bytes())).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let r = call(&mut a, "xpath_eval", json!({ "path": "i.pdf", "expression": "string(//ID)" }));
    assert_eq!(r["items"][0]["text"].as_str(), Some("\"RE-1\""), "{r}");
}

#[test]
fn an_open_pdf_is_edited_and_saved_as_an_update_that_keeps_the_file_before_it() {
    let dir = tempfile::tempdir().expect("dir");
    let original = pdf("factur-x.xml", INVOICE.as_bytes());
    std::fs::write(dir.path().join("i.pdf"), &original).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let opened = call(&mut a, "doc_open", json!({ "path": "i.pdf" }));
    let id = opened["id"].as_u64().expect("id");
    assert_eq!(call(&mut a, "doc_text", json!({ "id": id }))["text"].as_str(), Some(INVOICE));
    call(&mut a, "doc_edit", json!({ "id": id, "edits": [{ "line": 1, "column": 14, "end_line": 1, "end_column": 18, "text": "RE-2" }] }));
    call(&mut a, "doc_save", json!({ "id": id }));
    let saved = std::fs::read(dir.path().join("i.pdf")).expect("reads");
    assert!(saved.len() > original.len() && saved.starts_with(&original), "the original bytes first, the update after");
    let again = call(&mut a, "pdf_files", json!({ "path": "i.pdf", "extract": 0 }));
    assert_eq!(again["extracted"]["text"].as_str(), Some("<Invoice><ID>RE-2</ID></Invoice>"), "{again}");
    // Saved without a change since: the same bytes.
    call(&mut a, "doc_save", json!({ "id": id }));
    assert_eq!(std::fs::read(dir.path().join("i.pdf")).expect("reads"), saved);
    // As an XML file of its own.
    call(&mut a, "doc_save", json!({ "id": id, "path": "i.xml" }));
    assert_eq!(std::fs::read_to_string(dir.path().join("i.xml")).expect("reads"), "<Invoice><ID>RE-2</ID></Invoice>");
}
