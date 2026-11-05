//! A PDF in the real shell: it opens as the XML embedded in it, and saving writes the PDF back with that XML replaced and nothing else
//! touched.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 900.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
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

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    let started = Instant::now();
    while h.state().problems().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the first analysis");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

fn file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("writes");
    path
}

const INVOICE: &str = "<Invoice><ID>RE-1</ID></Invoice>";

#[test]
fn a_pdf_opens_as_its_xml_and_saving_it_edited_appends_an_update() {
    let dir = tempfile::tempdir().expect("dir");
    let original = pdf("factur-x.xml", INVOICE.as_bytes());
    let path = file(dir.path(), "rechnung.pdf", &original);
    let mut h = shell();
    open(&mut h, &path);
    assert_eq!(h.state().session.active_document().expect("shown").editor().text(), INVOICE);
    assert!(h.state().status.contains("factur-x.xml"), "{}", h.state().status);
    // Saving without an edit writes the same file.
    assert!(h.state_mut().execute("file.save"));
    h.run_steps(3);
    assert_eq!(std::fs::read(&path).expect("reads"), original);
    // An edit is saved as an incremental update.
    {
        let doc = h.state_mut().session.active_document_mut().expect("shown");
        doc.editor_mut().set_selection(13, 17);
        doc.editor_mut().type_text("RE-2");
    }
    assert!(h.state_mut().execute("file.save"));
    h.run_steps(3);
    let saved = std::fs::read(&path).expect("reads");
    assert!(saved.len() > original.len() && saved.starts_with(&original), "the original bytes first, the update after");
    let reread = xmlcraft_pdf::Pdf::open(saved).expect("a PDF");
    let data = reread.embedded_data(&reread.embedded_files()[0]).expect("data");
    assert_eq!(String::from_utf8(data).expect("utf-8"), "<Invoice><ID>RE-2</ID></Invoice>");
    assert!(!h.state().session.active_document().expect("shown").is_dirty());
}

#[test]
fn a_pdf_with_no_xml_is_refused_and_a_pdf_that_cannot_be_changed_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    assert!(!h.state_mut().open_path(&file(dir.path(), "x.pdf", b"not a pdf")));
    h.run_steps(2);
    assert!(h.state().session.documents().is_empty(), "nothing was opened");
    // Damaged: the cross-reference is gone, so the PDF is read by scanning and is not one to change.
    let mut damaged = pdf("factur-x.xml", INVOICE.as_bytes());
    let at = damaged.windows(9).rposition(|w| w == b"startxref").expect("there");
    damaged.truncate(at);
    open(&mut h, &file(dir.path(), "d.pdf", &damaged));
    assert!(h.state().status.contains("cannot be changed"), "{}", h.state().status);
}
