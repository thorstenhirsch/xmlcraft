//! A PDF file: its cross-references, and its objects by number.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::PdfError;
use crate::filter;
use crate::object::{Dict, Marks, ObjRef, Object, Parser, Stream, find, rfind};

/// The most cross-reference entries (objects) read.
const MAX_OBJECTS: usize = 10_000_000;
/// The most references followed (an array of references to arrays of references must not multiply) to read the filters and the
/// decode parameters of one stream.
const MAX_RESOLUTIONS: usize = 256;
/// The most sections of the cross-reference chain.
const MAX_SECTIONS: usize = 4096;
/// The most objects in one object stream.
const MAX_IN_STREAM: usize = 1_000_000;

#[derive(Clone, Copy, Debug)]
enum Entry {
    Free,
    InFile { offset: usize, generation: u16 },
    InStream { stream: u32, index: u32 },
}

/// The objects of object streams, by the number of the stream.
type StreamCache = HashMap<u32, Arc<Vec<(u32, Object)>>>;

/// Marks an object stream as being read, and unmarks it when dropped.
struct ReadingGuard<'a> {
    set: &'a Mutex<HashSet<u32>>,
    number: u32,
}

impl<'a> ReadingGuard<'a> {
    /// `None` when the stream is being read already (or the lock is poisoned).
    fn enter(set: &'a Mutex<HashSet<u32>>, number: u32) -> Option<Self> {
        // (`then`, not `then_some`: a guard built for a refused entry would remove the entry of the one that holds it.)
        let entered = set.lock().ok()?.insert(number);
        entered.then(|| Self { set, number })
    }
}

impl Drop for ReadingGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.number);
        }
    }
}

/// A PDF file, read.
pub struct Pdf {
    data: Vec<u8>,
    entries: HashMap<u32, Entry>,
    trailer: Dict,
    recovered: bool,
    /// The objects of the object streams read so far, by the number of the stream.
    streams: Mutex<StreamCache>,
    /// The object streams being read now (an object stream whose own filters are in itself would otherwise be read forever).
    reading: Mutex<HashSet<u32>>,
    /// Where `endstream` and `endobj` are.
    marks: Marks,
}

impl Pdf {
    /// Read the structure of a PDF. The bytes are kept as they are.
    pub fn open(data: Vec<u8>) -> Result<Pdf, PdfError> {
        let head = data.get(..data.len().min(1024)).unwrap_or_default();
        if find(head, b"%PDF-", 0).is_none() {
            return Err(PdfError::NotAPdf);
        }
        let marks = Marks::of(&data);
        let mut pdf = Pdf {
            data,
            entries: HashMap::new(),
            trailer: Dict::default(),
            recovered: false,
            streams: Mutex::new(HashMap::new()),
            reading: Mutex::new(HashSet::new()),
            marks,
        };
        let read = pdf.read_xref_chain();
        let usable = read.is_ok() && pdf.catalog_ref().is_some_and(|r| pdf.get(r).and_then(|o| o.as_dict().map(|_| ())).is_some());
        if !usable {
            pdf.recover();
        }
        if pdf.trailer.get("Encrypt").is_some() {
            return Err(PdfError::Encrypted);
        }
        if pdf.catalog_ref().is_none() {
            return Err(PdfError::Corrupt("there is no document catalog".to_owned()));
        }
        Ok(pdf)
    }

    /// The bytes of the file.
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    /// The trailer of the newest cross-reference section.
    pub fn trailer(&self) -> &Dict {
        &self.trailer
    }

    /// The cross-reference was damaged and the objects were found by scanning the file (a file that was read this way is not changed).
    pub fn recovered(&self) -> bool {
        self.recovered
    }

    /// The highest object number in use plus one (what a new object gets, and the `/Size` of an updated trailer).
    pub fn next_object_number(&self) -> u32 {
        let from_entries = self.entries.keys().max().map_or(0, |n| n.saturating_add(1));
        let size = self.trailer.get("Size").and_then(Object::as_int).and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
        from_entries.max(size)
    }

    pub(crate) fn catalog_ref(&self) -> Option<ObjRef> {
        self.trailer.get("Root").and_then(Object::as_ref)
    }

    /// The object a reference names. A reference whose generation is not that of the object is stale (the object was freed and the
    /// number reused) and names nothing, as the specification has it.
    pub fn get(&self, r: ObjRef) -> Option<Object> {
        match self.entries.get(&r.number)? {
            Entry::InFile { generation, .. } if *generation != r.generation => None,
            Entry::InStream { .. } if r.generation != 0 => None,
            _ => self.get_number(r.number),
        }
    }

    /// The object with this number, whatever generation it has.
    pub(crate) fn get_number(&self, number: u32) -> Option<Object> {
        match *self.entries.get(&number)? {
            Entry::Free => None,
            Entry::InFile { offset, .. } => self.object_at(offset, Some(number)),
            Entry::InStream { stream, index } => {
                let objects = self.object_stream(stream)?;
                objects.get(usize::try_from(index).ok()?).filter(|(n, _)| *n == number).map(|(_, o)| o.clone()).or_else(|| {
                    // The index may be wrong in a file whose entries were found by scanning: look the number up.
                    objects.iter().find(|(n, _)| *n == number).map(|(_, o)| o.clone())
                })
            }
        }
    }

    /// The offset of the object in the file when it is an ordinary one (not in an object stream).
    pub fn offset_of(&self, number: u32) -> Option<usize> {
        match self.entries.get(&number)? {
            Entry::InFile { offset, .. } => Some(*offset),
            _ => None,
        }
    }

    fn object_at(&self, offset: usize, expect: Option<u32>) -> Option<Object> {
        let mut parser = Parser::with_marks(&self.data, offset, &self.marks);
        let header = parser.object_header()?;
        if expect.is_some_and(|n| n != header.number) {
            return None;
        }
        let length = |dict: &Dict| -> Option<usize> {
            match dict.get("Length")? {
                Object::Int(n) => usize::try_from(*n).ok(),
                Object::Ref(r) => self.length_of(*r),
                _ => None,
            }
        };
        parser.object(&length)
    }

    /// The value of an indirect `/Length` (an integer object that is not itself a stream).
    fn length_of(&self, r: ObjRef) -> Option<usize> {
        let Entry::InFile { offset, generation } = *self.entries.get(&r.number)? else { return None };
        if generation != r.generation {
            return None;
        }
        let mut parser = Parser::with_marks(&self.data, offset, &self.marks);
        parser.object_header()?;
        match parser.object(&|_| None)? {
            Object::Int(n) => usize::try_from(n).ok(),
            _ => None,
        }
    }

    /// The object a value is: a reference is followed (a chain of them too, up to a bound).
    pub fn resolve(&self, object: &Object) -> Object {
        let mut current = object.clone();
        for _ in 0..16 {
            match current {
                Object::Ref(r) => match self.get(r) {
                    Some(next) => current = next,
                    None => return Object::Null,
                },
                other => return other,
            }
        }
        Object::Null
    }

    /// The decoded bytes of a stream.
    pub fn stream_data(&self, stream: &Stream) -> Result<Vec<u8>, PdfError> {
        let raw = self.data.get(stream.data.clone()).ok_or_else(|| PdfError::Corrupt("a stream is outside the file".to_owned()))?;
        // The references in the filter entries are followed (an array of filters, or parameters, may be indirect).
        let mut dict = stream.dict.clone();
        let mut budget = MAX_RESOLUTIONS;
        for (key, value) in &mut dict.entries {
            if matches!(key.as_str(), "Filter" | "F" | "DecodeParms" | "DP") {
                *value = self.resolve_deep(value, 0, &mut budget)?;
            }
        }
        filter::decode(raw, &dict)
    }

    /// `object` with every reference in it followed (to a depth); each reference followed is paid for from `budget`, and when it is
    /// spent the stream is refused rather than read with some of its parameters left out.
    fn resolve_deep(&self, object: &Object, depth: usize, budget: &mut usize) -> Result<Object, PdfError> {
        let resolved = match object {
            Object::Ref(_) => {
                if *budget == 0 {
                    return Err(PdfError::LimitExceeded("a stream's filters refer to more than 256 objects".to_owned()));
                }
                *budget -= 1;
                self.resolve(object)
            }
            other => other.clone(),
        };
        if depth > 4 {
            return Ok(resolved);
        }
        Ok(match resolved {
            Object::Array(items) => Object::Array(items.iter().map(|o| self.resolve_deep(o, depth + 1, budget)).collect::<Result<_, _>>()?),
            Object::Dict(d) => Object::Dict(Dict {
                entries: d.entries.iter().map(|(k, v)| Ok((k.clone(), self.resolve_deep(v, depth + 1, budget)?))).collect::<Result<_, PdfError>>()?,
            }),
            other => other,
        })
    }

    fn object_stream(&self, number: u32) -> Option<Arc<Vec<(u32, Object)>>> {
        if let Some(found) = self.streams.lock().ok()?.get(&number) {
            return Some(Arc::clone(found));
        }
        // An object stream is an ordinary object of the file: one that a cross-reference entry puts inside another stream (or itself)
        // is not one, and following it would never end.
        let Entry::InFile { offset, .. } = *self.entries.get(&number)? else { return None };
        // A stream whose own filters are objects inside it would be read forever: it is read once at a time.
        let _reading = ReadingGuard::enter(&self.reading, number)?;
        let Object::Stream(stream) = self.object_at(offset, Some(number))? else { return None };
        let count = usize::try_from(stream.dict.get("N").and_then(|n| self.resolve(n).as_int())?).ok().filter(|n| *n <= MAX_IN_STREAM)?;
        let first = usize::try_from(stream.dict.get("First").and_then(|n| self.resolve(n).as_int())?).ok()?;
        let data = self.stream_data(&stream).ok()?;
        let mut header = Parser::new(&data, 0);
        let mut places = Vec::with_capacity(count);
        for _ in 0..count {
            let n = u32::try_from(header.unsigned()?).ok()?;
            let offset = usize::try_from(header.unsigned()?).ok()?;
            places.push((n, offset));
        }
        let mut objects = Vec::with_capacity(count);
        for (n, offset) in places {
            let mut parser = Parser::new(&data, first.checked_add(offset)?);
            let object = parser.object(&|_| None)?;
            objects.push((n, object));
        }
        let objects = Arc::new(objects);
        self.streams.lock().ok()?.insert(number, Arc::clone(&objects));
        Some(objects)
    }

    // --- the cross-reference chain ---------------------------------------------------------------------------------------------

    fn read_xref_chain(&mut self) -> Result<(), PdfError> {
        let at = rfind(&self.data, b"startxref").ok_or_else(|| PdfError::Corrupt("there is no startxref".to_owned()))?;
        let mut parser = Parser::new(&self.data, at + "startxref".len());
        let start = usize::try_from(parser.unsigned().ok_or_else(|| PdfError::Corrupt("startxref has no offset".to_owned()))?)
            .map_err(|_| PdfError::Corrupt("startxref is out of range".to_owned()))?;
        let mut next = Some(start);
        let mut seen: HashSet<usize> = HashSet::new();
        let mut first = true;
        while let Some(offset) = next.take() {
            if !seen.insert(offset) {
                return Err(PdfError::Corrupt("the cross-reference sections lead back to themselves".to_owned()));
            }
            if seen.len() > MAX_SECTIONS {
                return Err(PdfError::LimitExceeded("the cross-reference has more than 4096 sections".to_owned()));
            }
            let (trailer, hybrid, freed) = self.read_section(offset, &HashSet::new())?;
            // A hybrid file's table is followed by the entries of its cross-reference stream before the older sections. The table lists
            // the objects the stream holds as free: for those, the stream says what they are.
            if let Some(h) = hybrid
                && seen.insert(h)
            {
                self.read_section(h, &freed)?;
            }
            next = match trailer.get("Prev") {
                None => None,
                Some(p) => Some(
                    p.as_int()
                        .and_then(|n| usize::try_from(n).ok())
                        .ok_or_else(|| PdfError::Corrupt("a trailer's /Prev is not an offset".to_owned()))?,
                ),
            };
            if self.entries.len() > MAX_OBJECTS {
                return Err(PdfError::LimitExceeded("the file has more than 10 million objects".to_owned()));
            }
            if first {
                self.trailer = trailer;
                first = false;
            }
        }
        Ok(())
    }

    /// Read the cross-reference section at `offset`: add its entries (those already known are newer and stay) and return its trailer and
    /// the offset of a hybrid file's `/XRefStm`.
    fn read_section(&mut self, offset: usize, replaces_free: &HashSet<u32>) -> Result<(Dict, Option<usize>, HashSet<u32>), PdfError> {
        let bad = |why: &str| PdfError::Corrupt(format!("the cross-reference at {offset} {why}"));
        let mut parser = Parser::new(&self.data, offset);
        if parser.keyword("xref") {
            let mut added: Vec<(u32, Entry)> = Vec::new();
            loop {
                parser.skip_space();
                let save = parser.pos;
                let Some(first) = parser.unsigned() else { break };
                let Some(count) = parser.unsigned() else {
                    parser.pos = save;
                    break;
                };
                let (first, count) = (
                    u32::try_from(first).map_err(|_| bad("has a bad subsection"))?,
                    usize::try_from(count).map_err(|_| bad("has a bad subsection"))?,
                );
                if count > MAX_OBJECTS || added.len().saturating_add(count) > MAX_OBJECTS {
                    return Err(PdfError::LimitExceeded(format!("the cross-reference at {offset} has more than 10 million entries")));
                }
                for i in 0..count {
                    let at = parser.unsigned().ok_or_else(|| bad("has a bad entry"))?;
                    let generation = u16::try_from(parser.unsigned().ok_or_else(|| bad("has a bad entry"))?).unwrap_or(u16::MAX);
                    let kind = if parser.keyword("n") {
                        true
                    } else if parser.keyword("f") {
                        false
                    } else {
                        return Err(bad("has a bad entry"));
                    };
                    let number =
                        first.checked_add(u32::try_from(i).map_err(|_| bad("has too many entries"))?).ok_or_else(|| bad("has a bad number"))?;
                    let entry = match usize::try_from(at) {
                        Ok(offset) if kind && offset > 0 => Entry::InFile { offset, generation },
                        _ => Entry::Free,
                    };
                    added.push((number, entry));
                }
            }
            if !parser.keyword("trailer") {
                return Err(bad("has no trailer"));
            }
            let trailer = match parser.object(&|_| None) {
                Some(Object::Dict(d)) => d,
                _ => return Err(bad("has a bad trailer")),
            };
            let hybrid = trailer.get("XRefStm").and_then(Object::as_int).and_then(|n| usize::try_from(n).ok());
            // (The free entries are noted only for a hybrid file, whose stream may say what some of them are.)
            let mut freed = HashSet::new();
            for (number, entry) in added {
                if self.entries.contains_key(&number) {
                    continue;
                }
                if hybrid.is_some() && matches!(entry, Entry::Free) {
                    freed.insert(number);
                }
                self.entries.insert(number, entry);
            }
            return Ok((trailer, hybrid, freed));
        }
        // A cross-reference stream.
        let mut parser = Parser::with_marks(&self.data, offset, &self.marks);
        parser.object_header().ok_or_else(|| bad("is neither a table nor a stream"))?;
        let Some(Object::Stream(stream)) = parser.object(&|d| d.get("Length").and_then(Object::as_int).and_then(|n| usize::try_from(n).ok())) else {
            return Err(bad("is not a stream"));
        };
        if stream.dict.get("Type").and_then(Object::as_name) != Some("XRef") {
            return Err(bad("is not a cross-reference stream"));
        }
        let data = self.stream_data(&stream)?;
        let widths: Vec<usize> = stream
            .dict
            .get("W")
            .and_then(Object::as_array)
            .map(|a| a.iter().filter_map(|o| o.as_int().and_then(|n| usize::try_from(n).ok())).collect())
            .unwrap_or_default();
        let [w0, w1, w2] = widths[..] else { return Err(bad("has a bad /W")) };
        if w0 > 8 || w1 > 8 || w2 > 8 || w0 + w1 + w2 == 0 {
            return Err(bad("has a bad /W"));
        }
        let size = stream.dict.get("Size").and_then(Object::as_int).and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
        let index: Vec<u32> = match stream.dict.get("Index").and_then(Object::as_array) {
            Some(a) => a.iter().filter_map(|o| o.as_int().and_then(|n| u32::try_from(n).ok())).collect(),
            None => vec![0, size],
        };
        let row = w0 + w1 + w2;
        let mut rows = data.chunks_exact(row);
        let field = |bytes: &[u8]| bytes.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
        for [first, count] in index.as_chunks::<2>().0.iter().copied() {
            for i in 0..count {
                let Some(r) = rows.next() else { return Err(bad("has fewer entries than its /Index says")) };
                let (kind, f1, f2) = (
                    if w0 == 0 { 1 } else { field(r.get(..w0).unwrap_or_default()) },
                    field(r.get(w0..w0 + w1).unwrap_or_default()),
                    field(r.get(w0 + w1..).unwrap_or_default()),
                );
                let Some(number) = first.checked_add(i) else { continue };
                let entry = match kind {
                    0 => Entry::Free,
                    1 => {
                        usize::try_from(f1).map_or(Entry::Free, |offset| Entry::InFile { offset, generation: u16::try_from(f2).unwrap_or(u16::MAX) })
                    }
                    2 => match (u32::try_from(f1), u32::try_from(f2)) {
                        (Ok(stream), Ok(index)) => Entry::InStream { stream, index },
                        _ => Entry::Free,
                    },
                    _ => continue,
                };
                if self.entries.len() > MAX_OBJECTS {
                    return Err(bad("has too many entries"));
                }
                match self.entries.get(&number) {
                    None => {
                        self.entries.insert(number, entry);
                    }
                    // An object the table of a hybrid file lists as free and this stream holds.
                    Some(Entry::Free) if replaces_free.contains(&number) => {
                        self.entries.insert(number, entry);
                    }
                    Some(_) => {}
                }
            }
        }
        Ok((stream.dict, None, HashSet::new()))
    }

    // --- recovery --------------------------------------------------------------------------------------------------------------

    /// Find the objects by scanning the file for `N G obj` and the trailer by scanning for `trailer` and cross-reference streams.
    fn recover(&mut self) {
        self.recovered = true;
        self.entries.clear();
        self.trailer = Dict::default();
        self.streams = Mutex::new(HashMap::new());
        let data = &self.data;
        let mut from = 0;
        let mut found: Vec<(u32, usize, u16)> = Vec::new();
        while let Some(at) = find(data, b"obj", from) {
            from = at + 3;
            let before_ok = at > 0 && data.get(at - 1).is_some_and(|b| crate::object::is_space(*b) || b.is_ascii_digit());
            let after_ok = !data.get(at + 3).is_some_and(|b| b.is_ascii_alphanumeric());
            if !(before_ok && after_ok) {
                continue;
            }
            // Walk back over `N G `.
            let mut i = at;
            let skip_space = |i: &mut usize| {
                while *i > 0 && data.get(*i - 1).is_some_and(|b| crate::object::is_space(*b)) {
                    *i -= 1;
                }
            };
            let digits = |i: &mut usize| {
                let end = *i;
                while *i > 0 && data.get(*i - 1).is_some_and(u8::is_ascii_digit) {
                    *i -= 1;
                }
                data.get(*i..end).and_then(|d| std::str::from_utf8(d).ok()).and_then(|d| d.parse::<u64>().ok())
            };
            skip_space(&mut i);
            let Some(generation) = digits(&mut i) else { continue };
            skip_space(&mut i);
            let Some(number) = digits(&mut i) else { continue };
            if let (Ok(number), Ok(generation)) = (u32::try_from(number), u16::try_from(generation)) {
                found.push((number, i, generation));
            }
            if found.len() > MAX_OBJECTS {
                break;
            }
        }
        for (number, offset, generation) in &found {
            self.entries.insert(*number, Entry::InFile { offset: *offset, generation: *generation });
        }
        // Trailers: the last `trailer` dictionary that has a /Root, or the dictionary of a cross-reference stream.
        let mut at = 0;
        while let Some(p) = find(&self.data, b"trailer", at) {
            at = p + 7;
            let mut parser = Parser::with_marks(&self.data, at, &self.marks);
            if let Some(Object::Dict(d)) = parser.object(&|_| None)
                && d.get("Root").is_some()
            {
                self.trailer = d;
            }
        }
        if self.trailer.get("Root").is_none() {
            for (number, _, _) in &found {
                if let Some(Object::Stream(s)) = self.get_number(*number)
                    && s.dict.get("Type").and_then(Object::as_name) == Some("XRef")
                    && s.dict.get("Root").is_some()
                {
                    self.trailer = s.dict.clone();
                }
            }
        }
        // The objects inside object streams.
        let streams: Vec<u32> = found
            .iter()
            .filter_map(|(n, _, _)| match self.get_number(*n) {
                Some(Object::Stream(s)) if s.dict.get("Type").and_then(Object::as_name) == Some("ObjStm") => Some(*n),
                _ => None,
            })
            .collect();
        for stream in streams {
            if let Some(objects) = self.object_stream(stream) {
                for (index, (n, _)) in objects.iter().enumerate() {
                    // (The objects of all the streams together are bounded as the objects of the file are.)
                    if self.entries.len() >= MAX_OBJECTS {
                        break;
                    }
                    if !matches!(self.entries.get(n), Some(Entry::InFile { .. })) {
                        self.entries.insert(*n, Entry::InStream { stream, index: u32::try_from(index).unwrap_or(0) });
                    }
                }
            }
        }
        // Without a trailer: the catalog is the object that says it is.
        if self.trailer.get("Root").is_none() {
            let catalog = found.iter().rev().find_map(|(n, _, generation)| {
                let r = ObjRef { number: *n, generation: *generation };
                match self.get(r)? {
                    Object::Dict(d) if d.get("Type").and_then(Object::as_name) == Some("Catalog") => Some(r),
                    _ => None,
                }
            });
            if let Some(r) = catalog {
                self.trailer.entries.push(("Root".to_owned(), Object::Ref(r)));
            }
        }
    }
}
