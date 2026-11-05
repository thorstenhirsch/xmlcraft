//! The BSON value model: every type of the specification, so that what is read can be written again byte for byte.

/// A document: the fields in the order they were written (names may repeat; BSON allows it).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Document {
    pub fields: Vec<(String, Bson)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Bson {
    Double(f64),
    String(String),
    Document(Document),
    Array(Vec<Bson>),
    Binary {
        subtype: u8,
        bytes: Vec<u8>,
    },
    /// Deprecated, but still found in old data.
    Undefined,
    ObjectId([u8; 12]),
    Bool(bool),
    /// Milliseconds since the Unix epoch.
    DateTime(i64),
    Null,
    Regex {
        pattern: String,
        options: String,
    },
    /// Deprecated: a namespace and an object id.
    DbPointer {
        namespace: String,
        id: [u8; 12],
    },
    JavaScript(String),
    /// Deprecated.
    Symbol(String),
    JavaScriptWithScope {
        code: String,
        scope: Document,
    },
    Int32(i32),
    Timestamp {
        time: u32,
        increment: u32,
    },
    Int64(i64),
    /// IEEE 754-2008 decimal128 in its BID encoding, little-endian as stored.
    Decimal128([u8; 16]),
    MinKey,
    MaxKey,
}

impl Bson {
    /// The type byte of the specification.
    pub fn type_byte(&self) -> u8 {
        match self {
            Bson::Double(_) => 0x01,
            Bson::String(_) => 0x02,
            Bson::Document(_) => 0x03,
            Bson::Array(_) => 0x04,
            Bson::Binary { .. } => 0x05,
            Bson::Undefined => 0x06,
            Bson::ObjectId(_) => 0x07,
            Bson::Bool(_) => 0x08,
            Bson::DateTime(_) => 0x09,
            Bson::Null => 0x0A,
            Bson::Regex { .. } => 0x0B,
            Bson::DbPointer { .. } => 0x0C,
            Bson::JavaScript(_) => 0x0D,
            Bson::Symbol(_) => 0x0E,
            Bson::JavaScriptWithScope { .. } => 0x0F,
            Bson::Int32(_) => 0x10,
            Bson::Timestamp { .. } => 0x11,
            Bson::Int64(_) => 0x12,
            Bson::Decimal128(_) => 0x13,
            Bson::MinKey => 0xFF,
            Bson::MaxKey => 0x7F,
        }
    }
}

/// Why BSON could not be read or a projection could not be turned into BSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BsonError {
    /// Where, as a byte offset of the bytes (reading) or of the text (a projection).
    pub offset: usize,
    /// How far the problem reaches in a text (a projection); zero for bytes.
    pub length: usize,
    pub message: String,
}

impl std::fmt::Display for BsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "at byte {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for BsonError {}
