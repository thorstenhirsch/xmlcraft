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

/// A PDF text string (UTF-16 with a byte-order mark, UTF-8 with one, or PDFDocEncoding, taken as Latin-1).
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
    bytes.iter().map(|b| char::from(*b)).collect()
}

impl Pdf {
    /// The files embedded in the document: those in the name tree and those in `/AF`, each once.
    pub fn embedded_files(&self) -> Vec<EmbeddedFile> {
        let mut specs: Vec<(Option<ObjRef>, Object)> = Vec::new();
        let Some(catalog) = self.catalog_ref().and_then(|r| self.get(r)) else { return Vec::new() };
        let Some(catalog) = catalog.as_dict() else { return Vec::new() };
        // The name tree.
        let tree = catalog.get("Names").map(|n| self.resolve(n)).and_then(|names| names.as_dict().and_then(|d| d.get("EmbeddedFiles").cloned()));
        if let Some(root) = tree {
            let mut seen = HashSet::new();
            self.collect_tree(&root, 0, &mut seen, &mut specs);
        }
        // The associated files.
        if let Some(Object::Array(items)) = catalog.get("AF").map(|a| self.resolve(a)) {
            for item in items.iter().take(MAX_FILES) {
                specs.push((item.as_ref(), self.resolve(item)));
            }
        }
        let mut found: Vec<EmbeddedFile> = Vec::new();
        let mut known: HashSet<ObjRef> = HashSet::new();
        for (reference, spec) in specs {
            let Some(file) = self.file_of(reference, &spec) else { continue };
            if known.insert(file.stream) {
                found.push(file);
            }
            if found.len() >= MAX_FILES {
                break;
            }
        }
        found
    }

    fn collect_tree(&self, node: &Object, depth: usize, seen: &mut HashSet<ObjRef>, out: &mut Vec<(Option<ObjRef>, Object)>) {
        if depth > MAX_TREE_DEPTH || out.len() >= MAX_FILES {
            return;
        }
        if let Some(r) = node.as_ref()
            && !seen.insert(r)
        {
            return;
        }
        let node = self.resolve(node);
        let Some(dict) = node.as_dict() else { return };
        if let Some(Object::Array(pairs)) = dict.get("Names").map(|n| self.resolve(n)) {
            for pair in pairs.chunks(2) {
                if let Some(spec) = pair.get(1) {
                    out.push((spec.as_ref(), self.resolve(spec)));
                }
            }
        }
        if let Some(Object::Array(kids)) = dict.get("Kids").map(|n| self.resolve(n)) {
            for kid in &kids {
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
        let stream = self.get(stream_ref)?;
        let stream_dict = stream.as_dict()?;
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
