//! Changing an embedded file by an **incremental update** (ISO 32000-1 §7.5.6): new objects and a new cross-reference section are
//! appended after the original bytes, so that every byte of the file as it was stays where it was, signatures over it stay as they were,
//! and a reader that stops at the older `startxref` sees the old file.

use crate::PdfError;
use crate::document::Pdf;
use crate::embedded::EmbeddedFile;
use crate::md5::{md5, md5_hex};
use crate::object::{Dict, ObjRef, Object};

/// The PDF date (`D:20261105123000Z`) of a time in seconds since 1970-01-01 UTC. A time outside the years chrono knows is the epoch.
pub fn pdf_date(unix_seconds: i64) -> String {
    let time = chrono::DateTime::from_timestamp(unix_seconds, 0).unwrap_or_default();
    time.format("D:%Y%m%d%H%M%SZ").to_string()
}

/// The bytes of an object as PDF syntax (a stream is written by the caller; here a stream is its dictionary).
fn write_object(object: &Object, out: &mut Vec<u8>, depth: usize) {
    if depth > 64 {
        out.extend_from_slice(b"null");
        return;
    }
    match object {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Object::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Object::Real(r) => {
            // Without an exponent (PDF has none); a number that is not finite cannot be written.
            let text = if r.is_finite() { format!("{r:.6}").trim_end_matches('0').trim_end_matches('.').to_owned() } else { "0".to_owned() };
            out.extend_from_slice(if text.is_empty() || text == "-" { b"0" } else { text.as_bytes() });
        }
        Object::String(bytes) => {
            out.push(b'<');
            for b in bytes {
                out.extend_from_slice(format!("{b:02X}").as_bytes());
            }
            out.push(b'>');
        }
        Object::Name(name) => write_name(name, out),
        Object::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_object(item, out, depth + 1);
            }
            out.push(b']');
        }
        Object::Dict(dict) => write_dict(dict, out, depth),
        Object::Stream(stream) => write_dict(&stream.dict, out, depth),
        Object::Ref(r) => out.extend_from_slice(format!("{} {} R", r.number, r.generation).as_bytes()),
    }
}

fn write_name(name: &str, out: &mut Vec<u8>) {
    out.push(b'/');
    for &b in name.as_bytes() {
        let plain = b.is_ascii_graphic() && !matches!(b, b'#' | b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%');
        if plain {
            out.push(b);
        } else {
            out.extend_from_slice(format!("#{b:02X}").as_bytes());
        }
    }
}

fn write_dict(dict: &Dict, out: &mut Vec<u8>, depth: usize) {
    out.extend_from_slice(b"<<");
    for (key, value) in &dict.entries {
        write_name(key, out);
        out.push(b' ');
        write_object(value, out, depth + 1);
        out.push(b' ');
    }
    out.extend_from_slice(b">>");
}

fn set(dict: &mut Dict, key: &str, value: Object) {
    dict.entries.retain(|(k, _)| k != key);
    dict.entries.push((key.to_owned(), value));
}

fn remove(dict: &mut Dict, key: &str) {
    dict.entries.retain(|(k, _)| k != key);
}

/// Whether the document cannot be changed without breaking something: it is signed or certified.
fn is_protected(pdf: &Pdf) -> Option<&'static str> {
    let catalog = pdf.catalog_ref().and_then(|r| pdf.get(r))?;
    let catalog = catalog.as_dict()?;
    if catalog.get("Perms").is_some() {
        return Some("it is certified (it has document permissions)");
    }
    if let Some(form) = catalog.get("AcroForm").map(|f| pdf.resolve(f))
        && let Some(form) = form.as_dict()
        && form.get("SigFlags").map(|f| pdf.resolve(f)).and_then(|f| f.as_int()).is_some_and(|f| f != 0)
    {
        return Some("it has digital signatures");
    }
    None
}

impl Pdf {
    /// Why the document would not be changed (it is damaged, encrypted, signed or certified), or none.
    pub fn why_not_editable(&self) -> Option<String> {
        if self.recovered() {
            return Some("its cross-reference is damaged, and XmlCraft does not repair PDFs".to_owned());
        }
        is_protected(self).map(|why| format!("{why}, and a change would break it"))
    }

    /// The whole file with the data of `file` replaced by `data`: the original bytes followed by an incremental update that holds the new
    /// stream (with `/Size`, `/ModDate` and `/CheckSum` of its `/Params` made right) and a new cross-reference section. `modified` is the
    /// PDF date of the change (`D:20261105123000Z`). Nothing before the update is touched.
    pub fn replace_embedded(&self, file: &EmbeddedFile, data: &[u8], modified: &str) -> Result<Vec<u8>, PdfError> {
        if let Some(why) = self.why_not_editable() {
            return Err(PdfError::Unsupported(format!("this PDF is not changed: {why}")));
        }
        if !modified.starts_with("D:") || !modified.is_ascii() || modified.contains(['(', ')', '\\']) || modified.len() > 40 {
            return Err(PdfError::Corrupt("the modification date is not a PDF date".to_owned()));
        }
        let Some(Object::Stream(old)) = self.get(file.stream) else {
            return Err(PdfError::Corrupt("the stream of the embedded file is missing".to_owned()));
        };
        let offset =
            self.offset_of(file.stream.number).ok_or_else(|| PdfError::Unsupported("the embedded file is inside an object stream".to_owned()))?;
        let previous = self.startxref().ok_or_else(|| PdfError::Corrupt("the file has no startxref".to_owned()))?;
        let stream_number = file.stream.number;
        let _ = offset;

        // The new stream's dictionary: the old one, with the data's own length, filter, and parameters.
        let mut dict = old.dict.clone();
        let deflate = old_filter_is_flate(&self.resolve_dict_value(&old.dict, "Filter"));
        let (body, filtered) = if deflate { (miniz_oxide::deflate::compress_to_vec_zlib(data, 6), true) } else { (data.to_vec(), false) };
        remove(&mut dict, "Filter");
        remove(&mut dict, "DecodeParms");
        remove(&mut dict, "F");
        remove(&mut dict, "DP");
        if filtered {
            set(&mut dict, "Filter", Object::Name("FlateDecode".to_owned()));
        }
        let mut params = match self.resolve_dict_value(&old.dict, "Params") {
            Object::Dict(p) => p,
            _ => Dict::default(),
        };
        set(&mut params, "Size", Object::Int(i64::try_from(data.len()).unwrap_or(i64::MAX)));
        set(&mut params, "ModDate", Object::String(modified.as_bytes().to_vec()));
        set(&mut params, "CheckSum", Object::String(md5(data).to_vec()));
        set(&mut dict, "Params", Object::Dict(params));
        set(&mut dict, "Length", Object::Int(i64::try_from(body.len()).unwrap_or(i64::MAX)));

        let mut out = self.bytes().to_vec();
        if !out.ends_with(b"\n") && !out.ends_with(b"\r") {
            out.push(b'\n');
        }
        let stream_at = out.len();
        out.extend_from_slice(format!("{stream_number} {} obj\n", file.stream.generation).as_bytes());
        write_dict(&dict, &mut out, 0);
        out.extend_from_slice(b"\nstream\n");
        out.extend_from_slice(&body);
        out.extend_from_slice(b"\nendstream\nendobj\n");

        // The trailer's entries that carry over, with a new second file identifier (the first stays: it names the original).
        let trailer = self.trailer();
        let size = self.next_object_number();
        let mut carried = Dict::default();
        for key in ["Root", "Info"] {
            if let Some(value) = trailer.get(key) {
                carried.entries.push((key.to_owned(), value.clone()));
            }
        }
        if let Some(Object::Array(ids)) = trailer.get("ID").map(|i| self.resolve(i)) {
            let first = ids.first().cloned().unwrap_or(Object::String(Vec::new()));
            let mut seed = self.bytes().len().to_le_bytes().to_vec();
            seed.extend_from_slice(md5_hex(data).as_bytes());
            seed.extend_from_slice(modified.as_bytes());
            carried.entries.push(("ID".to_owned(), Object::Array(vec![first, Object::String(md5(&seed).to_vec())])));
        }

        let table_at = out.len();
        if self.previous_is_stream() {
            // The previous section is a cross-reference stream: so is this one (its own entry is in it).
            let own = size;
            let rows: [(u32, usize); 2] = [(stream_number, stream_at), (own, table_at)];
            let mut data_rows: Vec<u8> = Vec::new();
            for (_, at) in rows {
                data_rows.push(1);
                data_rows.extend_from_slice(
                    &u32::try_from(at).map_err(|_| PdfError::Corrupt("the file is too large to update".to_owned()))?.to_be_bytes(),
                );
                data_rows.extend_from_slice(&0u16.to_be_bytes());
            }
            let mut xref = carried.clone();
            set(&mut xref, "Type", Object::Name("XRef".to_owned()));
            set(&mut xref, "Size", Object::Int(i64::from(own) + 1));
            set(&mut xref, "W", Object::Array(vec![Object::Int(1), Object::Int(4), Object::Int(2)]));
            set(
                &mut xref,
                "Index",
                Object::Array(vec![Object::Int(i64::from(stream_number)), Object::Int(1), Object::Int(i64::from(own)), Object::Int(1)]),
            );
            set(&mut xref, "Prev", Object::Int(i64::try_from(previous).unwrap_or(0)));
            set(&mut xref, "Length", Object::Int(i64::try_from(data_rows.len()).unwrap_or(0)));
            out.extend_from_slice(format!("{own} 0 obj\n").as_bytes());
            write_dict(&xref, &mut out, 0);
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(&data_rows);
            out.extend_from_slice(b"\nendstream\nendobj\n");
        } else {
            out.extend_from_slice(format!("xref\n{stream_number} 1\n{stream_at:010} {:05} n \n", file.stream.generation).as_bytes());
            let mut trailer_dict = carried;
            set(&mut trailer_dict, "Size", Object::Int(i64::from(size.max(stream_number.saturating_add(1)))));
            set(&mut trailer_dict, "Prev", Object::Int(i64::try_from(previous).unwrap_or(0)));
            out.extend_from_slice(b"trailer\n");
            write_dict(&trailer_dict, &mut out, 0);
            out.push(b'\n');
        }
        out.extend_from_slice(format!("startxref\n{table_at}\n%%EOF\n").as_bytes());
        Ok(out)
    }

    fn resolve_dict_value(&self, dict: &Dict, key: &str) -> Object {
        dict.get(key).map_or(Object::Null, |v| self.resolve(v))
    }
}

/// Whether the (resolved) `/Filter` is Flate and nothing else.
fn old_filter_is_flate(filter: &Object) -> bool {
    match filter {
        Object::Name(n) => matches!(n.as_str(), "FlateDecode" | "Fl"),
        Object::Array(items) => items.len() == 1 && items.first().is_some_and(old_filter_is_flate),
        _ => false,
    }
}

impl ObjRef {
    /// The reference as text, `5 0 R`.
    pub fn to_text(self) -> String {
        format!("{} {} R", self.number, self.generation)
    }
}
