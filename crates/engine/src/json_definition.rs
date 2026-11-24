//! Go to definition for `$ref` in JSON, JSONC, JSON5, BSON (Extended JSON) and YAML documents (JSON Schema and OpenAPI), and the JSON
//! Pointer of a place in a data document.
//!
//! A reference is a URI reference whose fragment is a JSON Pointer (RFC 6901, `#/definitions/x`, with `~0` and `~1`, percent-decoded).
//! A reference with a file part (`other.json#/x`) is read through the resolver, relative to the document: the open documents, the bundled
//! schemas, files (inside the root when the session has one) and, when the session works online, the network, like every other
//! reference of a grammar. Only one hop is resolved, so no cycle can loop. `$id`- and `$anchor`-based references are not resolved.

use xmlcraft_json_syntax::{Data, Dialect, Options, Segment, Value, parse, parse_lines};
use xmlcraft_resolve::{Budget, Request, Resolver};
use xmlcraft_text::Buffer;
use xmlcraft_text::encoding::{decode, sniff};

use crate::analysis::MAX_ANALYSED_BYTES;
use crate::language::{JsonFlavor, Language};
use crate::xsd_definition::{Definition, DefinitionKind};

/// The values a text holds (one, or one to a line, or one to a YAML document), as far as they could be read.
fn read_roots(text: &str, language: Language) -> Vec<Value> {
    match language {
        Language::Json(JsonFlavor::Lines) => {
            parse_lines(text, &Options { dialect: Dialect::Json, ..Options::default() }).into_iter().filter_map(|(_, read)| read.root).collect()
        }
        Language::Json(flavor) => parse(text, &Options { dialect: flavor.dialect(), ..Options::default() }).root.into_iter().collect(),
        Language::Bson => match xmlcraft_bson::layout_of_text(text) {
            xmlcraft_bson::Layout::Lines => read_roots(text, Language::Json(JsonFlavor::Lines)),
            xmlcraft_bson::Layout::Single => read_roots(text, Language::Json(JsonFlavor::Json)),
        },
        Language::Yaml => xmlcraft_yaml_syntax::parse(text, &xmlcraft_yaml_syntax::Options::default()).documents,
        Language::Xml | Language::Html | Language::XQuery => Vec::new(),
    }
}

/// The root that holds `offset` and the path to the innermost value at it.
fn path_at(roots: &[Value], offset: usize) -> Option<(&Value, Vec<Segment>)> {
    roots.iter().find_map(|root| root.path_at(offset).map(|path| (root, path)))
}

/// The JSON Pointer (RFC 6901) of the value at `offset` of a JSON, JSONC, JSON5, BSON or YAML document (relative to the line, for
/// JSON Lines, and to the document of a YAML stream), `""` for the root; none for another language, a text that is too large or
/// a place outside every value.
pub fn json_pointer_at(text: &str, language: Language, offset: usize) -> Option<String> {
    if !language.is_data() || text.len() > MAX_ANALYSED_BYTES {
        return None;
    }
    let roots = read_roots(text, language);
    let (_, mut path) = path_at(&roots, offset)?;
    if language == Language::Bson {
        // A type wrapper (`{"$numberInt": "5"}`) is one value: the pointer stops at it.
        if let Some(at) = path.iter().position(|s| matches!(s, Segment::Key(k) if xmlcraft_bson::is_wrapper_name(k))) {
            path.truncate(at);
        }
    }
    Some(pointer_of(&path))
}

/// The pointer for a path: `~` is `~0` and `/` is `~1` in a key.
fn pointer_of(path: &[Segment]) -> String {
    let mut out = String::new();
    for segment in path {
        out.push('/');
        match segment {
            Segment::Index(i) => out.push_str(&i.to_string()),
            Segment::Key(key) => {
                for c in key.chars() {
                    match c {
                        '~' => out.push_str("~0"),
                        '/' => out.push_str("~1"),
                        other => out.push(other),
                    }
                }
            }
        }
    }
    out
}

/// `%41` read as the bytes it stands for; none when a `%` is not followed by two hexadecimal digits or the result is not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while let Some(&b) = bytes.get(at) {
        if b == b'%' {
            let digits = text.get(at + 1..at + 3)?;
            out.push(u8::from_str_radix(digits, 16).ok()?);
            at += 3;
        } else {
            out.push(b);
            at += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Where the `$ref` at `offset` of the document `text` (at `location`, if it has one) points, if there is one there. An `Err` says why a
/// reference there cannot be followed (the file is not available, the pointer names nothing); `Ok(None)` is a place that is no `$ref`.
pub fn definition(language: Language, text: &str, location: &str, offset: usize, resolver: &Resolver) -> Result<Option<Definition>, String> {
    if !language.is_data() {
        return Ok(None);
    }
    if text.len() > MAX_ANALYSED_BYTES {
        return Err("the text is too large to analyse".to_owned());
    }
    let roots = read_roots(text, language);
    let Some((root, path)) = path_at(&roots, offset) else { return Ok(None) };
    if !matches!(path.last(), Some(Segment::Key(key)) if key == "$ref") {
        return Ok(None);
    }
    let Some(Data::String(reference)) = root.pointer(&pointer_of(&path)).map(|v| &v.data) else { return Ok(None) };
    let (file, fragment) = reference.split_once('#').unwrap_or((reference.as_str(), ""));
    let pointer =
        percent_decode(fragment).ok_or_else(|| format!("the reference {reference} has a % that is not followed by two hexadecimal digits"))?;
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(format!(
            "the reference {reference} names a location by a plain name (an anchor), which is not resolved; JSON Pointer fragments (#/…) are"
        ));
    }
    // The document the pointer is followed in: this one, or the file the reference names.
    let (target_location, target_text, target_language, target_roots);
    if file.is_empty() {
        (target_location, target_text, target_language) = (location.to_owned(), text.to_owned(), language);
        target_roots = roots.clone();
    } else {
        let found = resolver.resolve(&Request::uri(file, location), &mut Budget::default()).map_err(|f| f.message)?;
        if found.bytes.len() > MAX_ANALYSED_BYTES {
            return Err(format!("{} is too large to analyse", found.location));
        }
        let decoded = decode(&found.bytes, &sniff(&found.bytes));
        if decoded.lossy {
            return Err(format!("{}: it has bytes that are not valid in its encoding", found.location));
        }
        let kind = match Language::of_name(&found.location) {
            Language::Yaml => Language::Yaml,
            // (Comments and trailing commas are read, as for a schema: editors allow them.)
            _ => Language::Json(JsonFlavor::Json5),
        };
        target_roots = read_roots(&decoded.text, kind);
        (target_location, target_text, target_language) = (found.location, decoded.text, kind);
    }
    let _ = target_language;
    let Some(target_root) = target_roots.first() else { return Err(format!("{} holds no value", shown(&target_location))) };
    let (start, end) = if pointer.is_empty() {
        (target_root.span.start, target_root.span.start)
    } else {
        let Some(place) = target_root.span_of_pointer(&pointer) else {
            return Err(format!("the reference {reference} points to {pointer}, which does not exist in {}", shown(&target_location)));
        };
        inside_quotes(&target_text, place)
    };
    let selected = target_text.get(start..end).unwrap_or_default().to_owned();
    let buffer = Buffer::from_text(&target_text);
    let (from, to) = (buffer.position(start), buffer.position(end));
    Ok(Some(Definition {
        kind: DefinitionKind::Reference,
        name: reference.clone(),
        selected,
        location: if file.is_empty() { location.to_owned() } else { target_location },
        start,
        end,
        line: from.line,
        column: from.col,
        end_line: to.line,
        end_column: to.col,
    }))
}

fn shown(location: &str) -> &str {
    if location.is_empty() { "this document" } else { location }
}

/// A key's place without its quotes (a value's place stays as it is).
fn inside_quotes(text: &str, place: std::ops::Range<usize>) -> (usize, usize) {
    let Some(slice) = text.get(place.clone()) else { return (place.start, place.start) };
    for quote in ['"', '\''] {
        if slice.len() >= 2 && slice.starts_with(quote) && slice.ends_with(quote) {
            return (place.start + 1, place.end - 1);
        }
    }
    (place.start, place.end)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use xmlcraft_resolve::{Buffers, Environment, FileAccess, FixedClock, Policy};

    use super::*;

    struct Files(HashMap<String, Vec<u8>>);

    impl FileAccess for Files {
        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            self.0.get(path).cloned().ok_or_else(|| format!("{path}: no such file"))
        }
    }

    struct NoBuffers;

    impl Buffers for NoBuffers {
        fn text(&self, _: &str) -> Option<Arc<str>> {
            None
        }
    }

    fn resolver(files: &[(&str, &str)]) -> Resolver {
        let files = Files(files.iter().map(|(p, t)| ((*p).to_owned(), t.as_bytes().to_vec())).collect());
        let env = Environment { buffers: Arc::new(NoBuffers), files: Arc::new(files), fetcher: None, cache: None, clock: Arc::new(FixedClock(0)) };
        Resolver::new(env, Policy { auto_download: false, ..Policy::default() }, &[])
    }

    const JSON: Language = Language::Json(JsonFlavor::Json);

    /// The definition of the reference whose text contains `needle` (the place is inside it).
    fn go(language: Language, text: &str, needle: &str, files: &[(&str, &str)]) -> Result<Option<Definition>, String> {
        let at = text.find(needle).expect("the needle") + 1;
        definition(language, text, "/w/a.json", at, &resolver(files))
    }

    #[test]
    fn a_local_reference_goes_to_the_key_of_its_target() {
        let text = r##"{"definitions": {"pet": {"type": "object"}}, "properties": {"p": {"$ref": "#/definitions/pet"}}}"##;
        let found = go(JSON, text, "#/definitions/pet", &[]).expect("runs").expect("a definition");
        assert_eq!(found.selected, "pet");
        assert_eq!(text.get(found.start..found.end), Some("pet"), "the key without its quotes");
        assert_eq!((found.kind, found.name.as_str(), found.location.as_str()), (DefinitionKind::Reference, "#/definitions/pet", "/w/a.json"));
        assert_eq!((found.line, found.column), (0, text.find("pet").expect("key")));
        // The place may also be on the name `$ref`.
        let on_name = go(JSON, text, "$ref", &[]).expect("runs").expect("a definition");
        assert_eq!(on_name.start, found.start);
    }

    #[test]
    fn pointer_segments_are_unescaped_and_percent_decoded_and_array_items_are_found() {
        let text = r##"{"a/b": {"c~d": {"e f": 1}}, "list": [{"x": 1}, {"y": 2}], "r1": {"$ref": "#/a~1b/c~0d/e%20f"}, "r2": {"$ref": "#/list/1"}}"##;
        let found = go(JSON, text, "#/a~1b", &[]).expect("runs").expect("found");
        assert_eq!(found.selected, "e f");
        let item = go(JSON, text, "#/list/1", &[]).expect("runs").expect("found");
        assert_eq!(item.selected, "{\"y\": 2}", "an array item is selected as a whole");
        // The whole document.
        let whole = r##"{"self": {"$ref": "#"}}"##;
        let root = go(JSON, whole, "\"#\"", &[]).expect("runs").expect("found");
        assert_eq!((root.start, root.end), (0, 0));
    }

    #[test]
    fn yaml_references_are_followed_and_so_are_the_ones_of_other_documents() {
        let yaml = "components:\n  schemas:\n    Pet:\n      type: object\npaths:\n  /p:\n    get:\n      schema:\n        $ref: '#/components/schemas/Pet'\n";
        let found = go(Language::Yaml, yaml, "#/components", &[]).expect("runs").expect("found");
        assert_eq!(found.selected, "Pet");
        assert_eq!(yaml.get(found.start..found.end), Some("Pet"));
        // A file next to the document, JSON or YAML, read through the resolver.
        let text = r##"{"x": {"$ref": "defs.json#/shared/id"}, "y": {"$ref": "other.yaml#/k"}}"##;
        let files = [("/w/defs.json", "{\"shared\": {\"id\": {\"type\": \"string\"}}}"), ("/w/other.yaml", "k: 1\n")];
        let other = go(JSON, text, "defs.json", &files).expect("runs").expect("found");
        assert_eq!((other.location.as_str(), other.selected.as_str()), ("/w/defs.json", "id"));
        let yaml_file = go(JSON, text, "other.yaml", &files).expect("runs").expect("found");
        assert_eq!((yaml_file.location.as_str(), yaml_file.selected.as_str()), ("/w/other.yaml", "k"));
    }

    #[test]
    fn what_cannot_be_followed_says_why_and_a_place_that_is_no_reference_is_none() {
        let text =
            r##"{"a": 1, "r": {"$ref": "#/nothing/here"}, "f": {"$ref": "missing.json#/x"}, "n": {"$ref": "#anchor"}, "p": {"$ref": "#/a%zz"}}"##;
        let why = go(JSON, text, "#/nothing", &[]).expect_err("no such pointer");
        assert!(why.contains("#/nothing/here") && why.contains("does not exist"), "{why}");
        let file = go(JSON, text, "missing.json", &[]).expect_err("no such file");
        assert!(file.contains("missing.json"), "{file}");
        assert!(go(JSON, text, "#anchor", &[]).expect_err("anchors").contains("anchor"));
        assert!(go(JSON, text, "#/a%zz", &[]).expect_err("bad escape").contains('%'));
        // Not a $ref: a plain string, a number, a key, outside any value.
        for place in ["\"a\"", "1,", "\"r\""] {
            assert_eq!(go(JSON, text, place, &[]), Ok(None), "{place}");
        }
        assert_eq!(definition(JSON, text, "", text.len() + 5, &resolver(&[])), Ok(None));
        assert_eq!(definition(Language::Xml, "<a/>", "", 1, &resolver(&[])), Ok(None));
    }

    #[test]
    fn a_reference_to_a_reference_is_one_hop_and_a_cycle_does_not_loop() {
        let text = r##"{"a": {"$ref": "#/b"}, "b": {"$ref": "#/a"}}"##;
        let first = go(JSON, text, "#/b", &[]).expect("runs").expect("found");
        assert_eq!(first.selected, "b");
        let second = go(JSON, text, "#/a", &[]).expect("runs").expect("found");
        assert_eq!(second.selected, "a");
    }

    #[test]
    fn large_and_odd_documents_are_bounded() {
        let big = format!("{{\"a\": [{}], \"r\": {{\"$ref\": \"#/a/0\"}}}}", "0,".repeat(200_000) + "0");
        let at = big.find("#/a/0").expect("the reference");
        let found = definition(JSON, &big, "", at, &resolver(&[])).expect("runs").expect("found");
        assert_eq!(found.selected, "0");
        // Deep nesting is bounded by the reader, a broken text still answers.
        let deep = format!("{}{}", "[".repeat(5000), "]".repeat(5000));
        assert!(definition(JSON, &deep, "", 10, &resolver(&[])).is_ok());
        assert!(definition(Language::Yaml, "a: [1\n", "", 3, &resolver(&[])).is_ok());
        for text in ["", "{", "{\"$ref\":", "{\"$ref\": 5}", "{\"$ref\": \"é#/ü\"}", "[{\"$ref\": \"#/0\"}]"] {
            for at in 0..=text.len() {
                if text.is_char_boundary(at) {
                    let _ = definition(JSON, text, "", at, &resolver(&[]));
                }
            }
        }
    }

    #[test]
    fn the_pointer_of_a_place_escapes_keys_and_counts_items() {
        let text = "{\"a/b\": [10, {\"c~d\": true}], \"e\": null}";
        let at = |needle: &str| text.find(needle).expect("needle");
        assert_eq!(json_pointer_at(text, JSON, at("10")).as_deref(), Some("/a~1b/0"));
        assert_eq!(json_pointer_at(text, JSON, at("true")).as_deref(), Some("/a~1b/1/c~0d"));
        assert_eq!(json_pointer_at(text, JSON, at("\"c~d\"")).as_deref(), Some("/a~1b/1/c~0d"), "a name is its value's place");
        assert_eq!(json_pointer_at(text, JSON, 0).as_deref(), Some(""), "the root is the empty pointer");
        assert_eq!(json_pointer_at(text, JSON, text.len() + 1), None);
        let yaml = "a:\n  - x: 1\n    y: [2, 3]\n";
        assert_eq!(json_pointer_at(yaml, Language::Yaml, yaml.find('3').expect("3")).as_deref(), Some("/a/0/y/1"));
        let lines = "{\"a\": 1}\n{\"b\": [7]}\n";
        assert_eq!(json_pointer_at(lines, Language::Json(JsonFlavor::Lines), lines.find('7').expect("7")).as_deref(), Some("/b/0"));
        assert_eq!(json_pointer_at("<a/>", Language::Xml, 1), None);
        // BSON's text is Extended JSON: the pointer is the one of the text.
        assert_eq!(json_pointer_at("{\"n\": {\"$numberInt\": \"5\"}}", Language::Bson, 20).as_deref(), Some("/n"));
    }
}
