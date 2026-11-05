//! The files embedded in a PDF (ISO 32000-1 §7.11.4, ISO 32000-2 §14.13 and PDF/A-3's associated files): in the `/EmbeddedFiles` name
//! tree of the catalog and in its `/AF` array.

use std::collections::HashSet;

use crate::PdfError;
use crate::document::Pdf;
use crate::object::{Dict, ObjRef, Object};

/// The deepest a name tree is followed.
const MAX_TREE_DEPTH: usize = 16;
/// The most embedded files listed.
const MAX_FILES: usize = 10_000;

/// A file embedded in a PDF.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedFile {
    /// The file name (`/UF`, else `/F`).
    pub name: String,
    pub description: Option<String>,
    /// How the file relates to the document (`/AFRelationship`: `Data`, `Source`, `Alternative`, …).
    pub relationship: Option<String>,
    /// The media type (`/Subtype` of the stream: `text/xml`).
    pub mime: Option<String>,
    /// The size the stream's `/Params` declare (not checked against the data).
    pub size: Option<u64>,
    /// The file specification dictionary, when it is an indirect object.
    pub file_spec: Option<ObjRef>,
    /// The stream with the file's bytes.
    pub stream: ObjRef,
}

/// A PDF text string (UTF-16 with a byte-order mark, UTF-8 with one, or PDFDocEncoding).
pub fn text_string(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|p| u16::from_be_bytes(*p)).collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|p| u16::from_le_bytes(*p)).collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    bytes.iter().map(|b| pdf_doc_char(*b)).collect()
}

/// The character of a byte in PDFDocEncoding (ISO 32000-1 Annex D): Latin-1 but for the spacing accents at 0x18 to 0x1F, the typographic
/// marks at 0x80 to 0x9E and the euro at 0xA0; bytes with no character are U+FFFD.
fn pdf_doc_char(b: u8) -> char {
    const HIGH: [char; 31] = [
        '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}', '\u{2044}', '\u{2039}', '\u{203A}', '\u{2212}',
        '\u{2030}', '\u{201E}', '\u{201C}', '\u{201D}', '\u{2018}', '\u{2019}', '\u{201A}', '\u{2122}', '\u{FB01}', '\u{FB02}', '\u{0141}',
        '\u{0152}', '\u{0160}', '\u{0178}', '\u{017D}', '\u{0131}', '\u{0142}', '\u{0153}', '\u{0161}', '\u{017E}',
    ];
    const ACCENTS: [char; 8] = ['\u{02D8}', '\u{02C7}', '\u{02C6}', '\u{02D9}', '\u{02DD}', '\u{02DB}', '\u{02DA}', '\u{02DC}'];
    match b {
        0x18..=0x1F => ACCENTS.get(usize::from(b - 0x18)).copied().unwrap_or('\u{FFFD}'),
        0x80..=0x9E => HIGH.get(usize::from(b - 0x80)).copied().unwrap_or('\u{FFFD}'),
        0x7F | 0x9F | 0xAD => '\u{FFFD}',
        0xA0 => '\u{20AC}',
        other => char::from(other),
    }
}

impl Pdf {
    /// The files embedded in the document: those in the name tree and those in `/AF`, each file specification once.
    pub fn embedded_files(&self) -> Vec<EmbeddedFile> {
        // The file specifications as the document names them (references stay references until they are read, so that a document with
        // hundreds of thousands of entries costs only what is listed).
        let mut specs: Vec<Object> = Vec::new();
        let Some(catalog) = self.catalog_ref().and_then(|r| self.get(r)) else { return Vec::new() };
        let Some(catalog) = catalog.as_dict() else { return Vec::new() };
        let tree = catalog.get("Names").map(|n| self.resolve(n)).and_then(|names| names.as_dict().and_then(|d| d.get("EmbeddedFiles").cloned()));
        if let Some(root) = tree {
            let mut seen = HashSet::new();
            self.collect_tree(&root, 0, &mut seen, &mut specs);
        }
        if let Some(Object::Array(items)) = catalog.get("AF").map(|a| self.resolve(a)) {
            let room = MAX_FILES.saturating_sub(specs.len());
            specs.extend(items.into_iter().take(room));
        }
        let mut found: Vec<EmbeddedFile> = Vec::new();
        // A file specification is one file: found twice (the name tree and `/AF`) it is listed once, by its reference, or, for a
        // dictionary written in place, by its stream and name.
        let mut known_specs: HashSet<ObjRef> = HashSet::new();
        let mut known_direct: HashSet<(ObjRef, String)> = HashSet::new();
        for spec in specs {
            let reference = spec.as_ref();
            if let Some(r) = reference
                && !known_specs.insert(r)
            {
                continue;
            }
            let Some(file) = self.file_of(reference, &self.resolve(&spec)) else { continue };
            if reference.is_none() && !known_direct.insert((file.stream, file.name.clone())) {
                continue;
            }
            found.push(file);
        }
        found
    }

    fn collect_tree(&self, node: &Object, depth: usize, seen: &mut HashSet<u32>, out: &mut Vec<Object>) {
        if depth > MAX_TREE_DEPTH || out.len() >= MAX_FILES {
            return;
        }
        // (A node is visited once whatever generation a reference to it has: `get` is by number.)
        if let Some(r) = node.as_ref()
            && !seen.insert(r.number)
        {
            return;
        }
        let node = self.resolve(node);
        let Some(dict) = node.as_dict() else { return };
        if let Some(Object::Array(pairs)) = dict.get("Names").map(|n| self.resolve(n)) {
            for pair in pairs.chunks(2) {
                if out.len() >= MAX_FILES {
                    break;
                }
                if let Some(spec) = pair.get(1) {
                    out.push(spec.clone());
                }
            }
        }
        if let Some(Object::Array(kids)) = dict.get("Kids").map(|n| self.resolve(n)) {
            for kid in kids.iter().take(MAX_FILES) {
                self.collect_tree(kid, depth + 1, seen, out);
            }
        }
    }

    fn file_of(&self, reference: Option<ObjRef>, spec: &Object) -> Option<EmbeddedFile> {
        let dict: &Dict = spec.as_dict()?;
        let string = |key: &str| dict.get(key).map(|o| self.resolve(o)).and_then(|o| o.as_string().map(text_string));
        let embedded = dict.get("EF").map(|o| self.resolve(o))?;
        let embedded = embedded.as_dict()?;
        let stream_ref = embedded.get("UF").or_else(|| embedded.get("F")).and_then(Object::as_ref)?;
        // (A stream: a dictionary that is not one has no data.)
        let Object::Stream(stream) = self.get(stream_ref)? else { return None };
        let stream_dict = &stream.dict;
        let mime = stream_dict.get("Subtype").map(|s| self.resolve(s)).and_then(|s| s.as_name().map(str::to_owned));
        let size = stream_dict
            .get("Params")
            .map(|p| self.resolve(p))
            .and_then(|p| p.as_dict().and_then(|d| d.get("Size").map(|s| self.resolve(s))))
            .and_then(|s| s.as_int())
            .and_then(|n| u64::try_from(n).ok());
        Some(EmbeddedFile {
            name: string("UF").or_else(|| string("F")).unwrap_or_default(),
            description: string("Desc"),
            relationship: dict.get("AFRelationship").map(|o| self.resolve(o)).and_then(|o| o.as_name().map(str::to_owned)),
            mime,
            size,
            file_spec: reference,
            stream: stream_ref,
        })
    }

    /// The bytes of an embedded file (the stream decoded).
    pub fn embedded_data(&self, file: &EmbeddedFile) -> Result<Vec<u8>, PdfError> {
        match self.get(file.stream) {
            Some(Object::Stream(stream)) => self.stream_data(&stream),
            _ => Err(PdfError::Corrupt(format!("the stream of {} is missing", file.name))),
        }
    }
}
