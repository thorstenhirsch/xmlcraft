//! The tree a reading produces: values with their places in the text.

use std::ops::Range;

/// A value and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    /// The value in the text: for a string with its quotes, for an object or array from its opening to its closing bracket (to the end
    /// of what was read when it was never closed).
    pub span: Range<usize>,
    pub data: Data,
}

/// What a value is.
#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    Null,
    Bool(bool),
    /// The number as written (`-0.5e3`, and in JSON5 `0xFF` or `Infinity`): nothing is lost to a floating-point type.
    Number(String),
    /// The string with its escapes decoded.
    String(String),
    Array(Vec<Value>),
    Object(Vec<Member>),
}

/// One name and value of an object.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    /// The name with its escapes decoded.
    pub key: String,
    /// The name in the text, quotes included.
    pub key_span: Range<usize>,
    pub value: Value,
}

/// One step of a path into a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Key(String),
    Index(usize),
}

impl Value {
    /// The member of an object with this name (the first, if there are several).
    pub fn get(&self, key: &str) -> Option<&Value> {
        match &self.data {
            Data::Object(members) => members.iter().find(|m| m.key == key).map(|m| &m.value),
            _ => None,
        }
    }

    /// The value a JSON Pointer (RFC 6901, `/a/0/b`; the empty pointer is this value) names. A pointer that is not valid (a `~` not
    /// followed by `0` or `1`, an array index with a leading zero or a sign) names nothing.
    pub fn pointer(&self, pointer: &str) -> Option<&Value> {
        self.locate(pointer).map(|(_, value)| value)
    }

    /// The path to the innermost value that holds `offset`, from this value down (the empty path when only this value does); none when
    /// `offset` is outside it. A place in the name of a member is the member's value's path. In an object that repeats a name, the path names the member, but a pointer resolves a repeated name to its first member.
    pub fn path_at(&self, offset: usize) -> Option<Vec<Segment>> {
        if !(self.span.start <= offset && offset <= self.span.end) {
            return None;
        }
        let mut path = Vec::new();
        let mut current = self;
        loop {
            let next = match &current.data {
                Data::Object(members) => members
                    .iter()
                    .find(|m| m.key_span.start <= offset && offset <= m.value.span.end.max(m.key_span.end))
                    .map(|m| (Segment::Key(m.key.clone()), &m.value)),
                Data::Array(items) => {
                    items.iter().enumerate().find(|(_, v)| v.span.start <= offset && offset <= v.span.end).map(|(i, v)| (Segment::Index(i), v))
                }
                _ => None,
            };
            match next {
                Some((segment, value)) => {
                    path.push(segment);
                    current = value;
                }
                None => return Some(path),
            }
        }
    }

    /// The span of the value a JSON Pointer names, and of its name when it is a member of an object: what a problem about it is shown
    /// at (the name is the better place). Invalid pointers name nothing, as for [`Value::pointer`].
    pub fn span_of_pointer(&self, pointer: &str) -> Option<Range<usize>> {
        self.locate(pointer).map(|(place, _)| place)
    }

    /// The value a pointer names and the place to show it at.
    fn locate(&self, pointer: &str) -> Option<(Range<usize>, &Value)> {
        if pointer.is_empty() {
            return Some((self.span.clone(), self));
        }
        let mut current = self;
        let mut place = self.span.clone();
        for raw in pointer.strip_prefix('/')?.split('/') {
            let token = unescape_token(raw)?;
            match &current.data {
                Data::Object(members) => {
                    let m = members.iter().find(|m| m.key == token)?;
                    place = m.key_span.clone();
                    current = &m.value;
                }
                Data::Array(items) => {
                    let index = (token == "0" || !token.starts_with('0'))
                        .then(|| token.parse::<usize>().ok())
                        .flatten()
                        .filter(|_| token.bytes().all(|b| b.is_ascii_digit()))?;
                    let v = items.get(index)?;
                    place = v.span.clone();
                    current = v;
                }
                _ => return None,
            }
        }
        Some((place, current))
    }
}

/// A pointer token with `~1` and `~0` decoded; any other `~` makes the pointer invalid.
fn unescape_token(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '~' {
            match chars.next()? {
                '0' => out.push('~'),
                '1' => out.push('/'),
                _ => return None,
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}
