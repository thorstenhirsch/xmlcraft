//! Reading and writing BSON bytes.
//!
//! A `.bson` file is a series of documents, one after the other. Reading is strict about what makes the bytes ambiguous (lengths that
//! disagree, strings that are not UTF-8, arrays whose names are not 0, 1, 2 …) and says where; it is bounded in nesting and never
//! allocates more than the input is long.

use crate::model::{Bson, BsonError, Document};

/// Documents nested deeper than this are refused.
pub const MAX_DEPTH: usize = 100;

fn err<T>(offset: usize, message: impl Into<String>) -> Result<T, BsonError> {
    Err(BsonError { offset, length: 0, message: message.into() })
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&self, at: usize, n: usize) -> Result<&'a [u8], BsonError> {
        match at.checked_add(n).and_then(|end| self.bytes.get(at..end)) {
            Some(slice) => Ok(slice),
            None => err(at, "the bytes end here, in the middle of a value"),
        }
    }

    /// The `N` bytes at `at`.
    fn array<const N: usize>(&self, at: usize) -> Result<[u8; N], BsonError> {
        match self.take(at, N)?.try_into() {
            Ok(array) => Ok(array),
            Err(_) => err(at, "the bytes end here, in the middle of a value"),
        }
    }

    fn i32_at(&self, at: usize) -> Result<i32, BsonError> {
        Ok(i32::from_le_bytes(self.array(at)?))
    }

    fn cstring(&self, at: usize) -> Result<(String, usize), BsonError> {
        let rest = self.bytes.get(at..).unwrap_or_default();
        let Some(len) = rest.iter().position(|b| *b == 0) else { return err(at, "a name or text is not ended") };
        match std::str::from_utf8(rest.get(..len).unwrap_or_default()) {
            Ok(s) => Ok((s.to_owned(), at + len + 1)),
            Err(_) => err(at, "a name or text is not valid UTF-8"),
        }
    }

    fn string(&self, at: usize) -> Result<(String, usize), BsonError> {
        let len = self.i32_at(at)?;
        let len = usize::try_from(len).ok().filter(|l| *l >= 1);
        let Some(len) = len else { return err(at, "a string has a length below one") };
        let body = self.take(at + 4, len)?;
        let (last, text) = body.split_last().unwrap_or((&1, &[]));
        if *last != 0 {
            return err(at + 4 + len - 1, "a string is not ended with a zero byte");
        }
        match std::str::from_utf8(text) {
            Ok(s) => Ok((s.to_owned(), at + 4 + len)),
            Err(_) => err(at + 4, "a string is not valid UTF-8"),
        }
    }

    /// The document at `at`; returns it and where it ends.
    fn document(&self, at: usize, depth: usize) -> Result<(Document, usize), BsonError> {
        if depth > MAX_DEPTH {
            return err(at, format!("documents are nested deeper than {MAX_DEPTH} levels"));
        }
        let size = self.i32_at(at)?;
        let size = usize::try_from(size).ok().filter(|s| *s >= 5);
        let Some(size) = size else { return err(at, "a document has a length below five") };
        let end = at.checked_add(size).filter(|e| *e <= self.bytes.len());
        let Some(end) = end else { return err(at, "a document is longer than the bytes that are left") };
        if self.bytes.get(end - 1) != Some(&0) {
            return err(end - 1, "a document is not ended with a zero byte");
        }
        let mut fields = Vec::new();
        let mut pos = at + 4;
        loop {
            let Some(&kind) = self.bytes.get(pos) else { return err(pos, "the bytes end in the middle of a document") };
            if kind == 0 {
                if pos + 1 != end {
                    return err(pos, "a document ends before its length says");
                }
                return Ok((Document { fields }, end));
            }
            if pos + 1 >= end {
                return err(pos, "a field runs past the end of its document");
            }
            let (name, after_name) = self.cstring(pos + 1)?;
            let (value, after) = self.value(kind, after_name, depth)?;
            if after > end - 1 {
                return err(pos, "a value runs past the end of its document");
            }
            fields.push((name, value));
            pos = after;
        }
    }

    fn value(&self, kind: u8, at: usize, depth: usize) -> Result<(Bson, usize), BsonError> {
        Ok(match kind {
            0x01 => (Bson::Double(f64::from_le_bytes(self.array(at)?)), at + 8),
            0x02 => self.string(at).map(|(s, e)| (Bson::String(s), e))?,
            0x03 => self.document(at, depth + 1).map(|(d, e)| (Bson::Document(d), e))?,
            0x04 => {
                let (doc, end) = self.document(at, depth + 1)?;
                let mut items = Vec::with_capacity(doc.fields.len());
                for (i, (name, value)) in doc.fields.into_iter().enumerate() {
                    if name != i.to_string() {
                        return err(at, format!("an array has the name '{name}' where '{i}' is expected"));
                    }
                    items.push(value);
                }
                (Bson::Array(items), end)
            }
            0x05 => {
                let len = self.i32_at(at)?;
                let Some(len) = usize::try_from(len).ok() else { return err(at, "binary data has a negative length") };
                let subtype = *self.take(at + 4, 1)?.first().unwrap_or(&0);
                let bytes = self.take(at + 5, len)?.to_vec();
                (Bson::Binary { subtype, bytes }, at + 5 + len)
            }
            0x06 => (Bson::Undefined, at),
            0x07 => (Bson::ObjectId(self.array(at)?), at + 12),
            0x08 => match self.take(at, 1)?.first() {
                Some(0) => (Bson::Bool(false), at + 1),
                Some(1) => (Bson::Bool(true), at + 1),
                _ => return err(at, "a boolean is neither 0 nor 1"),
            },
            0x09 => (Bson::DateTime(i64::from_le_bytes(self.array(at)?)), at + 8),
            0x0A => (Bson::Null, at),
            0x0B => {
                let (pattern, next) = self.cstring(at)?;
                let (options, end) = self.cstring(next)?;
                (Bson::Regex { pattern, options }, end)
            }
            0x0C => {
                let (namespace, next) = self.string(at)?;
                (Bson::DbPointer { namespace, id: self.array(next)? }, next + 12)
            }
            0x0D => self.string(at).map(|(s, e)| (Bson::JavaScript(s), e))?,
            0x0E => self.string(at).map(|(s, e)| (Bson::Symbol(s), e))?,
            0x0F => {
                let total = self.i32_at(at)?;
                let Some(total) = usize::try_from(total).ok().filter(|t| *t >= 14) else { return err(at, "code with a scope is too short") };
                let end = at.checked_add(total).filter(|e| *e <= self.bytes.len());
                let Some(end) = end else { return err(at, "code with a scope is longer than the bytes that are left") };
                let (code, next) = self.string(at + 4)?;
                let (scope, scope_end) = self.document(next, depth + 1)?;
                if scope_end != end {
                    return err(at, "code with a scope has a length that does not fit");
                }
                (Bson::JavaScriptWithScope { code, scope }, end)
            }
            0x10 => (Bson::Int32(self.i32_at(at)?), at + 4),
            0x11 => {
                let increment = u32::from_le_bytes(self.array(at)?);
                let time = u32::from_le_bytes(self.array(at + 4)?);
                (Bson::Timestamp { time, increment }, at + 8)
            }
            0x12 => (Bson::Int64(i64::from_le_bytes(self.array(at)?)), at + 8),
            0x13 => (Bson::Decimal128(self.array(at)?), at + 16),
            0xFF => (Bson::MinKey, at),
            0x7F => (Bson::MaxKey, at),
            other => return err(at.saturating_sub(1), format!("the type 0x{other:02X} is not a BSON type")),
        })
    }
}

/// Read every document of `bytes` (a `.bson` file is a series of them). Empty bytes are no documents.
pub fn read_all(bytes: &[u8]) -> Result<Vec<Document>, BsonError> {
    let reader = Reader { bytes };
    let mut documents = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let (document, end) = reader.document(at, 0)?;
        documents.push(document);
        at = end;
    }
    Ok(documents)
}

fn put_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as i32 + 1).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

fn put_cstring(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}

fn put_document(out: &mut Vec<u8>, fields: &[(String, Bson)]) {
    let start = out.len();
    out.extend_from_slice(&[0; 4]);
    for (name, value) in fields {
        out.push(value.type_byte());
        put_cstring(out, name);
        put_value(out, value);
    }
    out.push(0);
    let size = (out.len() - start) as i32;
    if let Some(slot) = out.get_mut(start..start + 4) {
        slot.copy_from_slice(&size.to_le_bytes());
    }
}

fn put_value(out: &mut Vec<u8>, value: &Bson) {
    match value {
        Bson::Double(f) => out.extend_from_slice(&f.to_le_bytes()),
        Bson::String(s) | Bson::JavaScript(s) | Bson::Symbol(s) => put_string(out, s),
        Bson::Document(d) => put_document(out, &d.fields),
        Bson::Array(items) => {
            let fields: Vec<(String, Bson)> = items.iter().enumerate().map(|(i, v)| (i.to_string(), v.clone())).collect();
            put_document(out, &fields);
        }
        Bson::Binary { subtype, bytes } => {
            out.extend_from_slice(&(bytes.len() as i32).to_le_bytes());
            out.push(*subtype);
            out.extend_from_slice(bytes);
        }
        Bson::Undefined | Bson::Null | Bson::MinKey | Bson::MaxKey => {}
        Bson::ObjectId(id) => out.extend_from_slice(id),
        Bson::Bool(b) => out.push(u8::from(*b)),
        Bson::DateTime(ms) | Bson::Int64(ms) => out.extend_from_slice(&ms.to_le_bytes()),
        Bson::Regex { pattern, options } => {
            put_cstring(out, pattern);
            put_cstring(out, options);
        }
        Bson::DbPointer { namespace, id } => {
            put_string(out, namespace);
            out.extend_from_slice(id);
        }
        Bson::JavaScriptWithScope { code, scope } => {
            let start = out.len();
            out.extend_from_slice(&[0; 4]);
            put_string(out, code);
            put_document(out, &scope.fields);
            let size = (out.len() - start) as i32;
            if let Some(slot) = out.get_mut(start..start + 4) {
                slot.copy_from_slice(&size.to_le_bytes());
            }
        }
        Bson::Int32(n) => out.extend_from_slice(&n.to_le_bytes()),
        Bson::Timestamp { time, increment } => {
            out.extend_from_slice(&increment.to_le_bytes());
            out.extend_from_slice(&time.to_le_bytes());
        }
        Bson::Decimal128(d) => out.extend_from_slice(d),
    }
}

/// The bytes of `documents`, one after the other.
pub fn write_all(documents: &[Document]) -> Vec<u8> {
    let mut out = Vec::new();
    for document in documents {
        put_document(&mut out, &document.fields);
    }
    out
}
