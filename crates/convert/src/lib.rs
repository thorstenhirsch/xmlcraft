#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! Conversions between XML, JSON, YAML and BSON (as MongoDB Extended JSON) for XmlCraft.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web too.
//! - Data formats (JSON, YAML, BSON's text) all become the value tree of `xmlcraft-json-syntax`, so converting between them is reading
//!   and writing. What each target can hold is spelled out: JSON has no `NaN`, a YAML mapping key is a string here, a BSON text is
//!   Extended JSON (a plain document converts to it as it is; converting *from* it can drop the types, see [`extended_to_plain`]).
//! - XML to and from data has two mappings. The **natural** one ([`xml_to_value`], [`value_to_xml`]) reads elements as members, attributes
//!   as `@name`, mixed text as `#text` and repeated elements as arrays; it is readable and loses the interleaving of different elements.
//!   The **standard** one ([`standard_to_xml`], [`standard_to_value`]) is the `fn:json-to-xml` / `fn:xml-to-json` mapping of XPath 3.1,
//!   which is lossless for JSON.
//! - Never panics, bounded by the depth of what it reads; every refusal says what and where.

mod extended;
mod standard;
mod write;
mod xml;

use xmlcraft_json_syntax::{Dialect, Options as JsonOptions, Severity, Value, parse};

pub use extended::extended_to_plain;
pub use standard::{standard_to_value, standard_to_xml};
pub use write::{json_text, yaml_text};
pub use xml::{DataToXml, XmlToData, value_to_xml, xml_to_value};

/// A format a text can be in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Xml,
    Json,
    Yaml,
    /// The Extended JSON text of BSON documents.
    Bson,
}

impl Format {
    /// `xml`, `json`, `yaml` or `bson`.
    pub fn parse(name: &str) -> Option<Format> {
        Some(match name {
            "xml" => Format::Xml,
            "json" => Format::Json,
            "yaml" | "yml" => Format::Yaml,
            "bson" => Format::Bson,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Xml => "xml",
            Format::Json => "json",
            Format::Yaml => "yaml",
            Format::Bson => "bson",
        }
    }
}

/// Why a conversion was not done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConvertError {
    /// Where in the text that was read, when that is known (a byte offset).
    pub offset: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.offset {
            Some(at) => write!(f, "at byte {at}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ConvertError {}

fn error(offset: Option<usize>, message: impl Into<String>) -> ConvertError {
    ConvertError { offset, message: message.into() }
}

/// The most spaces of indentation a writer uses for a level (more is no more readable, and an input-sized number must not decide how
/// much is allocated).
pub(crate) const MAX_INDENT: usize = 16;

/// The value of a quoted attribute value as the syntax tree keeps it (quotes included): without the one pair of quotes around it, and
/// not without the quotes that belong to the value.
pub(crate) fn unquote(raw: &str) -> &str {
    let mut chars = raw.chars();
    match (chars.next(), chars.next_back()) {
        (Some(first), Some(last)) if (first == '"' || first == '\'') && first == last => raw.get(1..raw.len() - 1).unwrap_or_default(),
        _ => raw,
    }
}

/// How to convert.
#[derive(Clone, Debug)]
pub struct ConvertOptions {
    /// XML read as data.
    pub xml_to_data: XmlToData,
    /// Data written as XML.
    pub data_to_xml: DataToXml,
    /// Spaces of indentation in the output, at most 16 (`None`: JSON and XML on one line; YAML is always indented).
    pub indent: Option<usize>,
    /// JSON is read as JSON Lines: one value to a line, together an array.
    pub json_lines: bool,
    /// Use the standard `fn:json-to-xml` mapping for XML to and from JSON instead of the natural one.
    pub standard_mapping: bool,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self { xml_to_data: XmlToData::default(), data_to_xml: DataToXml::default(), indent: Some(2), json_lines: false, standard_mapping: false }
    }
}

/// What a conversion made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Converted {
    pub text: String,
    /// Things the target could not hold exactly, said once each.
    pub warnings: Vec<String>,
}

/// The text read as a value tree. A text with several values (YAML documents, BSON documents one to a line) gives an array of them.
pub fn read_value(text: &str, from: Format, options: &ConvertOptions) -> Result<Value, ConvertError> {
    match from {
        Format::Xml if options.standard_mapping => standard_to_value(text),
        Format::Xml => xml_to_value(text, &options.xml_to_data),
        Format::Json if options.json_lines => {
            let mut items = Vec::new();
            for (_, read) in xmlcraft_json_syntax::parse_lines(text, &JsonOptions { dialect: Dialect::Json5, ..JsonOptions::default() }) {
                if let Some(d) = read.diagnostics.iter().find(|d| d.severity() == Severity::Error) {
                    return Err(error(Some(d.span.start), d.message()));
                }
                items.extend(read.root);
            }
            Ok(Value { span: 0..0, data: xmlcraft_json_syntax::Data::Array(items) })
        }
        Format::Json => {
            let read = parse(text, &JsonOptions { dialect: Dialect::Json5, ..JsonOptions::default() });
            if let Some(d) = read.diagnostics.iter().find(|d| d.severity() == Severity::Error) {
                return Err(error(Some(d.span.start), d.message()));
            }
            read.root.ok_or_else(|| error(None, "the text has no value"))
        }
        Format::Yaml => {
            let read = xmlcraft_yaml_syntax::parse(text, &xmlcraft_yaml_syntax::Options::default());
            if let Some(d) = read.diagnostics.iter().find(|d| d.severity() == Severity::Error) {
                return Err(error(Some(d.span.start), d.message()));
            }
            Ok(one_or_all(read.documents))
        }
        Format::Bson => {
            // The text is Extended JSON; one document or one to a line.
            let documents = xmlcraft_bson::from_text(text).map_err(|errors| {
                let first = errors.first();
                error(first.map(|e| e.offset), first.map(|e| e.message.clone()).unwrap_or_default())
            })?;
            let (json, layout) = xmlcraft_bson::to_text(&documents, true).map_err(|e| error(None, e.message))?;
            let read = if layout == xmlcraft_bson::Layout::Lines {
                Value {
                    span: 0..0,
                    data: xmlcraft_json_syntax::Data::Array(
                        xmlcraft_json_syntax::parse_lines(&json, &JsonOptions::default()).into_iter().filter_map(|(_, r)| r.root).collect(),
                    ),
                }
            } else {
                parse(&json, &JsonOptions::default()).root.unwrap_or(Value { span: 0..0, data: xmlcraft_json_syntax::Data::Null })
            };
            Ok(read)
        }
    }
}

fn one_or_all(mut documents: Vec<Value>) -> Value {
    match documents.len() {
        1 => documents.remove(0),
        0 => Value { span: 0..0, data: xmlcraft_json_syntax::Data::Null },
        _ => Value { span: 0..0, data: xmlcraft_json_syntax::Data::Array(documents) },
    }
}

/// Convert `text` from one format to another.
pub fn convert(text: &str, from: Format, to: Format, options: &ConvertOptions) -> Result<Converted, ConvertError> {
    let mut value = read_value(text, from, options)?;
    let indent = options.indent.map(|n| n.min(MAX_INDENT));
    let mut warnings = Vec::new();
    // Out of BSON's text into a format that has no such types, the typed values become plain ones.
    if from == Format::Bson && to != Format::Bson {
        let (plain, changed) = extended::extended_to_plain_noting(&value);
        if changed {
            warnings.push("BSON types such as ObjectId, dates and 64-bit integers were written as plain values".to_owned());
        }
        value = plain;
    }
    let out = match to {
        Format::Json => json_text(&value, indent, &mut warnings)?,
        Format::Yaml => yaml_text(&value, &mut warnings),
        Format::Xml if options.standard_mapping => standard_to_xml(&value, indent, &mut warnings)?,
        Format::Xml => value_to_xml(&value, &options.data_to_xml, indent, &mut warnings)?,
        // The text of BSON: the data as Extended JSON, which any JSON is (plain numbers are int32, int64 or double by their size). A list
        // is several documents, one to a line; repeated names stay (BSON documents may have them).
        Format::Bson => {
            let documents: Vec<&Value> = match &value.data {
                xmlcraft_json_syntax::Data::Array(items) => items.iter().collect(),
                _ => vec![&value],
            };
            let mut lines = Vec::with_capacity(documents.len());
            for document in documents {
                lines.push(write::json_text_all(document, None, &mut warnings)?);
            }
            // (The offsets of a refusal are those of the text just written, not of the input: they are not given.)
            let documents = xmlcraft_bson::from_text(&lines.join("\n"))
                .map_err(|errors| error(None, errors.first().map(|e| e.message.clone()).unwrap_or_default()))?;
            xmlcraft_bson::to_text(&documents, true).map_err(|e| error(None, e.message))?.0
        }
    };
    Ok(Converted { text: out, warnings })
}
