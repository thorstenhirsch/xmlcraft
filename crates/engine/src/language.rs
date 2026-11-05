//! The language of a document: what the editor stack reads it as.
//!
//! XML (with its schema languages) is the first; the data formats follow it one by one. The language comes from the document's name,
//! so Save As under another extension changes how the text is coloured, outlined and checked.

use xmlcraft_format::{Edit, FormatError, FormatOptions};
use xmlcraft_json_syntax::Dialect;

/// How a JSON document is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JsonFlavor {
    /// Strict JSON (RFC 8259).
    Json,
    /// JSON with comments and trailing commas (`.jsonc`, and the configuration files that editors read this way).
    Jsonc,
    Json5,
    /// One JSON value per line (`.jsonl`, `.ndjson`).
    Lines,
}

impl JsonFlavor {
    /// The dialect each line (or the whole text) is read with.
    pub fn dialect(self) -> Dialect {
        match self {
            JsonFlavor::Json | JsonFlavor::Lines => Dialect::Json,
            JsonFlavor::Jsonc => Dialect::Jsonc,
            JsonFlavor::Json5 => Dialect::Json5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Xml,
    Json(JsonFlavor),
    /// A BSON file: shown and edited as its MongoDB Extended JSON text, saved as the bytes that text stands for.
    Bson,
}

/// File names (lower case) that are JSON with comments although they end in `.json`: the configuration files editors read that way.
const JSONC_NAMES: &[&str] = &[
    "jsconfig.json",
    "devcontainer.json",
    "settings.json",
    "launch.json",
    "tasks.json",
    "extensions.json",
    ".eslintrc.json",
    ".eslintrc",
    ".babelrc",
];

impl Language {
    /// The language of a document called `name` (anything that is not recognised is XML, as before).
    pub fn of_name(name: &str) -> Language {
        let file = name.rsplit(['/', '\\']).next().unwrap_or(name).to_ascii_lowercase();
        if JSONC_NAMES.contains(&file.as_str()) || (file.starts_with("tsconfig.") && file.ends_with(".json")) {
            return Language::Json(JsonFlavor::Jsonc);
        }
        match file.rsplit_once('.').map(|(_, ext)| ext) {
            Some("json" | "geojson" | "webmanifest" | "har" | "ipynb" | "jsonld" | "map") => Language::Json(JsonFlavor::Json),
            Some("jsonc") => Language::Json(JsonFlavor::Jsonc),
            Some("json5") => Language::Json(JsonFlavor::Json5),
            Some("jsonl" | "ndjson") => Language::Json(JsonFlavor::Lines),
            Some("bson") => Language::Bson,
            _ => Language::Xml,
        }
    }

    /// The edits that pretty-print `text` (or, with `minify`, strip its white space), in the way of this language. Only white space
    /// between tokens changes. JSON Lines are not indented (a value per line is the format): they can only be minified.
    pub fn format(self, text: &str, options: &FormatOptions, minify: bool) -> Result<Vec<Edit>, FormatError> {
        match self {
            Language::Xml if minify => xmlcraft_format::minify(text),
            Language::Xml => xmlcraft_format::pretty_print_settled(text, options),
            Language::Json(JsonFlavor::Lines) if minify => xmlcraft_format::json::minify_lines(text, Dialect::Json),
            Language::Json(JsonFlavor::Lines) => Err(FormatError::Refused {
                message: "JSON Lines have one value on each line, so they are not indented (minify keeps them as they are)".to_owned(),
            }),
            Language::Json(flavor) if minify => xmlcraft_format::json::minify(text, flavor.dialect()),
            Language::Json(flavor) => xmlcraft_format::json::pretty_print(text, flavor.dialect(), options),
            // A BSON file's text is one indented document, or one document to a line: formatted as the one it is.
            Language::Bson => match xmlcraft_bson::layout_of_text(text) {
                xmlcraft_bson::Layout::Lines => Language::Json(JsonFlavor::Lines).format(text, options, minify),
                xmlcraft_bson::Layout::Single => Language::Json(JsonFlavor::Json).format(text, options, minify),
            },
        }
    }

    pub fn is_xml(self) -> bool {
        self == Language::Xml
    }

    pub fn is_json(self) -> bool {
        matches!(self, Language::Json(_))
    }

    /// JSON in a form (a BSON file is shown as Extended JSON): coloured, folded and typed in as JSON.
    pub fn is_json_like(self) -> bool {
        matches!(self, Language::Json(_) | Language::Bson)
    }

    pub fn is_bson(self) -> bool {
        self == Language::Bson
    }

    /// The name used in the tools' answers.
    pub fn name(self) -> &'static str {
        match self {
            Language::Xml => "xml",
            Language::Json(JsonFlavor::Json) => "json",
            Language::Json(JsonFlavor::Jsonc) => "jsonc",
            Language::Json(JsonFlavor::Json5) => "json5",
            Language::Json(JsonFlavor::Lines) => "jsonl",
            Language::Bson => "bson",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_of_a_document_says_what_it_is() {
        let of = Language::of_name;
        assert_eq!(of("a.json"), Language::Json(JsonFlavor::Json));
        assert_eq!(of("A.GeoJSON"), Language::Json(JsonFlavor::Json));
        assert_eq!(of("/x/y/a.jsonc"), Language::Json(JsonFlavor::Jsonc));
        assert_eq!(of("C:\\x\\tsconfig.json"), Language::Json(JsonFlavor::Jsonc));
        assert_eq!(of("/p/.vscode/settings.json"), Language::Json(JsonFlavor::Jsonc));
        assert_eq!(of("tsconfig.app.json"), Language::Json(JsonFlavor::Jsonc));
        assert_eq!(of("a.json5"), Language::Json(JsonFlavor::Json5));
        assert_eq!(of("log.NDJSON"), Language::Json(JsonFlavor::Lines));
        assert_eq!(of("a.jsonl"), Language::Json(JsonFlavor::Lines));
        for xml in ["a.xml", "a.xsd", "a.dtd", "Untitled-1", "", "json", ".json.bak", "a.json.xml", "dir.json/a"] {
            assert_eq!(of(xml), Language::Xml, "{xml:?}");
        }
    }
}
