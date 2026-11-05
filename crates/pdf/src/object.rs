//! PDF objects and their syntax (ISO 32000-1 §7.3): what a PDF is made of, read from bytes at a place.

use std::ops::Range;

/// How deeply arrays and dictionaries may nest.
const MAX_DEPTH: usize = 64;
/// The most items in one array or dictionary.
const MAX_ITEMS: usize = 1_000_000;

/// A reference to an indirect object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjRef {
    pub number: u32,
    pub generation: u16,
}

/// A dictionary: names and objects, in the order they were written (a repeated name: the last is used by [`Dict::get`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dict {
    pub entries: Vec<(String, Object)>,
}

impl Dict {
    pub fn get(&self, key: &str) -> Option<&Object> {
        self.entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

/// A stream: its dictionary and where its (still encoded) bytes are in the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    pub dict: Dict,
    /// The raw bytes in the file (between `stream` and `endstream`).
    pub data: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Object {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    /// The bytes of a string (escapes and hexadecimal read).
    String(Vec<u8>),
    /// A name, without the `/` and with `#xx` read (bytes that are not UTF-8 are replaced).
    Name(String),
    Array(Vec<Object>),
    Dict(Dict),
    Stream(Stream),
    Ref(ObjRef),
}

impl Object {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Object::Int(i) => Some(*i),
            Object::Real(r) if r.fract() == 0.0 && r.is_finite() && r.abs() < 9.0e15 => Some(*r as i64),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&str> {
        if let Object::Name(n) = self { Some(n) } else { None }
    }

    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Object]> {
        if let Object::Array(a) = self { Some(a) } else { None }
    }

    pub fn as_ref(&self) -> Option<ObjRef> {
        if let Object::Ref(r) = self { Some(*r) } else { None }
    }

    pub fn as_string(&self) -> Option<&[u8]> {
        if let Object::String(s) = self { Some(s) } else { None }
    }
}

/// White space (§7.2.2): NUL, tab, line feed, form feed, carriage return, space.
pub(crate) fn is_space(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delimiter(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn is_regular(b: u8) -> bool {
    !is_space(b) && !is_delimiter(b)
}

/// Where `endstream` and `endobj` are in a file, found once, so that the end of a stream whose length cannot be trusted is found
/// without scanning the file again for each stream.
#[derive(Clone, Debug, Default)]
pub struct Marks {
    endstream: Vec<usize>,
    endobj: Vec<usize>,
    /// The `obj` keywords that open an object (not the end of `endobj`).
    objs: Vec<usize>,
}

impl Marks {
    pub fn of(data: &[u8]) -> Marks {
        let positions = |needle: &[u8]| {
            let mut out = Vec::new();
            let mut from = 0;
            while let Some(at) = find(data, needle, from) {
                out.push(at);
                from = at + needle.len();
            }
            out
        };
        let objs = positions(b"obj").into_iter().filter(|at| !data.get(at.saturating_sub(3)..*at).is_some_and(|before| before == b"end")).collect();
        Marks { endstream: positions(b"endstream"), endobj: positions(b"endobj"), objs }
    }

    /// The `endstream` that ends a stream whose data starts at `start`: the last one before the next `endobj` or the next object (the
    /// word can be in the data, but a stream does not run past its own object, even when it lacks its `endobj`).
    fn stream_end(&self, start: usize) -> Option<usize> {
        let next = |positions: &[usize]| positions.get(positions.partition_point(|p| *p < start)).copied().unwrap_or(usize::MAX);
        let limit = next(&self.endobj).min(next(&self.objs));
        let lo = self.endstream.partition_point(|p| *p < start);
        let hi = self.endstream.partition_point(|p| *p < limit);
        if hi > lo { self.endstream.get(hi - 1).copied() } else { None }
    }
}

/// Reads objects from bytes.
pub struct Parser<'a> {
    pub data: &'a [u8],
    pub pos: usize,
    marks: Option<&'a Marks>,
}

impl<'a> Parser<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos, marks: None }
    }

    /// A parser of a file whose [`Marks`] are known.
    pub fn with_marks(data: &'a [u8], pos: usize, marks: &'a Marks) -> Self {
        Self { data, pos, marks: Some(marks) }
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// Skip white space and comments.
    pub fn skip_space(&mut self) {
        while let Some(b) = self.peek() {
            if is_space(b) {
                self.pos += 1;
            } else if b == b'%' {
                while let Some(c) = self.peek() {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    /// The regular characters at the position (a keyword, a number).
    fn word(&mut self) -> &'a [u8] {
        let start = self.pos;
        while self.peek().is_some_and(is_regular) {
            self.pos += 1;
        }
        self.data.get(start..self.pos).unwrap_or_default()
    }

    /// Read a keyword (`obj`, `endobj`, `stream`, `xref`, `trailer`, `startxref`) if it is next.
    pub fn keyword(&mut self, word: &str) -> bool {
        self.skip_space();
        let save = self.pos;
        if self.word() == word.as_bytes() {
            true
        } else {
            self.pos = save;
            false
        }
    }

    /// A non-negative integer if one is next.
    pub fn unsigned(&mut self) -> Option<u64> {
        self.skip_space();
        let save = self.pos;
        let word = self.word();
        match std::str::from_utf8(word).ok().filter(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit())).and_then(|w| w.parse().ok()) {
            Some(n) => Some(n),
            None => {
                self.pos = save;
                None
            }
        }
    }

    /// `N G obj`: the reference the object at this place is.
    pub fn object_header(&mut self) -> Option<ObjRef> {
        let number = u32::try_from(self.unsigned()?).ok()?;
        let generation = u16::try_from(self.unsigned()?).ok()?;
        self.keyword("obj").then_some(ObjRef { number, generation })
    }

    /// An object; a stream is read when `stream` follows a dictionary. `length` says how long a stream is when it is not given
    /// directly (an indirect `/Length`).
    pub fn object(&mut self, length: &dyn Fn(&Dict) -> Option<usize>) -> Option<Object> {
        self.object_at(0, length)
    }

    fn object_at(&mut self, depth: usize, length: &dyn Fn(&Dict) -> Option<usize>) -> Option<Object> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.skip_space();
        let b = self.peek()?;
        match b {
            b'/' => Some(Object::Name(self.name())),
            b'(' => self.literal_string().map(Object::String),
            b'<' if self.data.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                let dict = self.dict_body(depth, length)?;
                // A stream follows `>>` as `stream` + a line break.
                let save = self.pos;
                self.skip_space();
                if self.word() == b"stream" {
                    return self.stream_body(dict, length);
                }
                self.pos = save;
                Some(Object::Dict(dict))
            }
            b'<' => self.hex_string().map(Object::String),
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_space();
                    if self.peek()? == b']' {
                        self.pos += 1;
                        return Some(Object::Array(items));
                    }
                    if items.len() >= MAX_ITEMS {
                        return None;
                    }
                    items.push(self.object_at(depth + 1, length)?);
                }
            }
            b')' | b']' | b'>' | b'{' | b'}' => None,
            _ => self.scalar(),
        }
    }

    fn dict_body(&mut self, depth: usize, length: &dyn Fn(&Dict) -> Option<usize>) -> Option<Dict> {
        let mut dict = Dict::default();
        loop {
            self.skip_space();
            match self.peek()? {
                b'>' => {
                    if self.data.get(self.pos + 1) == Some(&b'>') {
                        self.pos += 2;
                        return Some(dict);
                    }
                    return None;
                }
                b'/' => {
                    let key = self.name();
                    if dict.entries.len() >= MAX_ITEMS {
                        return None;
                    }
                    let value = self.object_at(depth + 1, length)?;
                    dict.entries.push((key, value));
                }
                _ => return None,
            }
        }
    }

    fn stream_body(&mut self, dict: Dict, length: &dyn Fn(&Dict) -> Option<usize>) -> Option<Object> {
        // `stream` is followed by CRLF or LF (a lone CR is tolerated).
        match (self.peek()?, self.data.get(self.pos + 1)) {
            (b'\r', Some(b'\n')) => self.pos += 2,
            (b'\n' | b'\r', _) => self.pos += 1,
            _ => {}
        }
        let start = self.pos;
        let declared = length(&dict).filter(|n| start.checked_add(*n).is_some_and(|end| end <= self.data.len()));
        let end = match declared {
            // The declared length counts if `endstream` follows it.
            Some(n)
                if self.data.get(start + n..).is_some_and(|rest| {
                    // (A few bytes of white space, not a run of any length.)
                    rest.iter().take(64).position(|b| !is_space(*b)).is_some_and(|i| rest.get(i..).is_some_and(|r| r.starts_with(b"endstream")))
                }) =>
            {
                start + n
            }
            // Else the last `endstream` before the next `endobj` ends it (and the line break before it is not data).
            _ => {
                let found = match self.marks {
                    Some(marks) => marks.stream_end(start)?,
                    // (Without the marks of a whole file, in the little data that is not one: look from here.)
                    None => Marks::of(self.data.get(start..)?).stream_end(0)? + start,
                };
                let mut end = found;
                if end > start && self.data.get(end - 1) == Some(&b'\n') {
                    end -= 1;
                }
                if end > start && self.data.get(end - 1) == Some(&b'\r') {
                    end -= 1;
                }
                end
            }
        };
        self.pos = end;
        self.skip_space();
        if self.word() != b"endstream" {
            return None;
        }
        Some(Object::Stream(Stream { dict, data: start..end }))
    }

    fn name(&mut self) -> String {
        self.pos += 1; // the slash
        let start = self.pos;
        while self.peek().is_some_and(is_regular) {
            self.pos += 1;
        }
        let raw = self.data.get(start..self.pos).unwrap_or_default();
        let mut bytes = Vec::with_capacity(raw.len());
        let mut i = 0;
        while let Some(&b) = raw.get(i) {
            let hex = |c: Option<&u8>| c.and_then(|c| (*c as char).to_digit(16));
            if b == b'#'
                && let (Some(h), Some(l)) = (hex(raw.get(i + 1)), hex(raw.get(i + 2)))
            {
                bytes.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
            bytes.push(b);
            i += 1;
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn literal_string(&mut self) -> Option<Vec<u8>> {
        self.pos += 1; // the parenthesis
        let mut out = Vec::new();
        let mut depth = 1usize;
        loop {
            let b = self.peek()?;
            self.pos += 1;
            match b {
                b'(' => {
                    depth += 1;
                    out.push(b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(out);
                    }
                    out.push(b);
                }
                b'\\' => {
                    let c = self.peek()?;
                    self.pos += 1;
                    match c {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            let mut value = u32::from(c - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        value = value * 8 + u32::from(d - b'0');
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push((value & 0xFF) as u8);
                        }
                        other => out.push(other),
                    }
                }
                // A line break inside a string is a line feed.
                b'\r' => {
                    if self.peek() == Some(b'\n') {
                        self.pos += 1;
                    }
                    out.push(b'\n');
                }
                _ => out.push(b),
            }
        }
    }

    fn hex_string(&mut self) -> Option<Vec<u8>> {
        self.pos += 1;
        let mut out = Vec::new();
        let mut high: Option<u8> = None;
        loop {
            let b = self.peek()?;
            self.pos += 1;
            match b {
                b'>' => {
                    if let Some(h) = high {
                        out.push(h << 4);
                    }
                    return Some(out);
                }
                _ if is_space(b) => {}
                _ => {
                    let d = (b as char).to_digit(16)? as u8;
                    match high.take() {
                        Some(h) => out.push((h << 4) | d),
                        None => high = Some(d),
                    }
                }
            }
        }
    }

    /// A number, `true`, `false`, `null`, or a reference `N G R`.
    fn scalar(&mut self) -> Option<Object> {
        let start = self.pos;
        let word = self.word();
        if word.is_empty() {
            return None;
        }
        match word {
            b"true" => return Some(Object::Bool(true)),
            b"false" => return Some(Object::Bool(false)),
            b"null" => return Some(Object::Null),
            _ => {}
        }
        let text = std::str::from_utf8(word).ok()?;
        if text.bytes().all(|b| b.is_ascii_digit()) {
            let number: i64 = text.parse().ok()?;
            // `N G R`?
            let after_number = self.pos;
            self.skip_space();
            let gen_start = self.pos;
            let generation = self.word();
            if !generation.is_empty() && generation.iter().all(u8::is_ascii_digit) {
                let after_generation = self.pos;
                self.skip_space();
                if self.peek() == Some(b'R') && !self.data.get(self.pos + 1).is_some_and(|b| is_regular(*b)) {
                    self.pos += 1;
                    let generation = std::str::from_utf8(generation).ok()?.parse::<u16>().ok()?;
                    return Some(Object::Ref(ObjRef { number: u32::try_from(number).ok()?, generation }));
                }
                self.pos = after_generation;
            }
            self.pos = after_number;
            let _ = gen_start;
            return Some(Object::Int(number));
        }
        // Signs and a point; PDF has no exponents.
        let body = text.strip_prefix(['+', '-']).unwrap_or(text);
        if !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit()) {
            return text.parse::<i64>().ok().map(Object::Int);
        }
        if !body.is_empty()
            && body.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            && body.bytes().filter(|b| *b == b'.').count() <= 1
            && body != "."
        {
            return text.parse::<f64>().ok().filter(|r| r.is_finite()).map(Object::Real);
        }
        // A word that is nothing of the above is not an object here (a keyword in the wrong place).
        self.pos = start;
        None
    }
}

/// The first place at or after `from` where `needle` is.
pub(crate) fn find(data: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let hay = data.get(from..)?;
    hay.windows(needle.len()).position(|w| w == needle).map(|i| from + i)
}

/// The last place where `needle` is.
pub(crate) fn rfind(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len()).rposition(|w| w == needle)
}
