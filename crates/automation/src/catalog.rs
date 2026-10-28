//! `catalog_lookup`: where XML catalogs map an identifier or a URI. The catalogs are files read through the root confinement; the
//! catalogs they name (`nextCatalog`, `delegate…`) are read the same way when they are `file:` URIs, and never fetched otherwise.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_catalog::{Catalog, Catalogs, Loader};
use xmlcraft_text::encoding::{decode, sniff};

use xmlcraft_platform::subset::path_of;

use crate::file_access::Confinement;
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The largest catalog read.
const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;
/// The most problems with the named catalogs one call lists.
const MAX_NOTES: usize = 100;
/// The most catalogs one call names.
const MAX_CATALOGS: usize = 64;

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "catalog_lookup",
        title: "Look up a catalog entry",
        description: "Where XML catalogs (OASIS XML Catalogs 1.1) map an external identifier (`public` and/or `system`) or a `uri` such as a namespace name: the answer, \
                      or null. `catalogs` are the catalog files in the order they are consulted; those they name (nextCatalog, delegate…) are read when they are \
                      files, and never downloaded. Where no `prefer` attribute says, a public identifier counts even with a system one unless `prefer_public` is false.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "catalogs": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": MAX_CATALOGS, "description": "Catalog files, in order" },
                "public": { "type": "string" },
                "system": { "type": "string" },
                "uri": { "type": "string", "description": "A URI to map, alone (not with public or system)" },
                "prefer_public": { "type": "boolean" },
            },
            "required": ["catalogs"],
            "additionalProperties": false,
        }),
        read_only: true,
        destructive: false,
        handler: catalog_lookup,
    }]
}

/// A `file:` URI for a path: every byte that is not a letter, a digit, one of `-._~` or a separator is written as a percent-escape, so
/// that a name with a `%`, a space or a backslash in it comes back as it was.
fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    // Windows separators are slashes in a URI, and a drive letter is a path segment (`file:///C:/dir`).
    #[cfg(windows)]
    let text = text.replace('\\', "/");
    let text = if text.starts_with('/') { text.to_string() } else { format!("/{text}") };
    let mut uri = String::from("file://");
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') || (cfg!(windows) && byte == b':') {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// Reads the catalogs that are named, through the confinement and only when they are files; keeps what it was told about the ones
/// that could not be used.
struct FileLoader<'a> {
    confinement: &'a Confinement,
    /// How many were read, in all.
    reads: usize,
    notes: Vec<String>,
}

impl FileLoader<'_> {
    fn read(&mut self, path: &str) -> Option<String> {
        let path = self.confinement.resolve_read(path).ok()?;
        // The size first: a file over the limit is not read into memory only to be refused.
        if std::fs::metadata(&path).ok()?.len() > MAX_CATALOG_BYTES as u64 {
            return None;
        }
        let loaded = xmlcraft_platform::files::read_file(&path).ok()?;
        if loaded.bytes.len() > MAX_CATALOG_BYTES {
            return None;
        }
        self.reads += 1;
        Some(decode(&loaded.bytes, &sniff(&loaded.bytes)).text)
    }
}

impl Loader for FileLoader<'_> {
    fn load(&mut self, location: &str) -> Option<String> {
        // Only a `file:` catalog is read: nothing is downloaded, and a path outside the root is not read.
        self.read(&path_of(location)?)
    }

    fn note(&mut self, location: &str, problem: &str) {
        // The list is bounded, and its last place says so, as the list of a catalog's own warnings does.
        if self.notes.len() + 1 < MAX_NOTES {
            self.notes.push(format!("{location}: {problem}"));
        } else if self.notes.len() + 1 == MAX_NOTES {
            self.notes.push(format!("more than {} problems with the named catalogs: the rest are not listed", MAX_NOTES - 1));
        }
    }
}

fn catalog_lookup(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let text = |key: &str| args.get(key).and_then(Value::as_str);
    let (public, system, uri) = (text("public"), text("system"), text("uri"));
    match (public.is_some() || system.is_some(), uri.is_some()) {
        (true, false) | (false, true) => {}
        _ => return Err(ToolError::InvalidArgs("give a uri, or a public and/or a system identifier".into())),
    }
    let names: Vec<&str> =
        args.get("catalogs").and_then(Value::as_array).map(|list| list.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if names.is_empty() || names.len() > MAX_CATALOGS {
        return Err(ToolError::InvalidArgs(format!("catalogs lists one to {MAX_CATALOGS} files")));
    }
    let mut loader = FileLoader { confinement: &a.confinement, reads: 0, notes: Vec::new() };
    let mut all = Catalogs::new();
    let mut warnings: Vec<String> = Vec::new();
    for name in names {
        let path = loader.confinement.resolve_read(name)?;
        let content = loader
            .read(&path.to_string_lossy())
            .ok_or_else(|| ToolError::Failed(format!("{name} cannot be read as a catalog (or is over {MAX_CATALOG_BYTES} bytes)")))?;
        let catalog = Catalog::parse(&content, &file_uri(&path)).map_err(|e| ToolError::Failed(format!("{name}: {e}")))?;
        warnings.extend(catalog.warnings().iter().map(|w| format!("{name}: {w}")));
        all.push(catalog);
    }
    if let Some(prefer) = args.get("prefer_public").and_then(Value::as_bool) {
        all.set_prefer_public(prefer);
    }
    let resolved = match uri {
        Some(uri) => all.resolve_uri(uri, &mut loader),
        None => all.resolve(public, system, &mut loader),
    };
    // A `file:` answer says whether the file is there.
    let exists = resolved.as_deref().and_then(path_of).map(|path| a.confinement.resolve_read(&path).is_ok_and(|real| real.is_file()));
    // What the catalogs that were named (not the ones given) had wrong with them comes after the others.
    warnings.extend(loader.notes.iter().cloned());
    Ok(vec![Content::Json(json!({ "resolved": resolved, "exists": exists, "catalogs_read": loader.reads, "warnings": warnings }))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_and_paths_go_both_ways() {
        assert_eq!(file_uri(Path::new("/etc/xml/cat alog.xml")), "file:///etc/xml/cat%20alog.xml");
        assert_eq!(path_of("file:///etc/xml/cat%20alog.xml").as_deref(), Some("/etc/xml/cat alog.xml"));
        // A name with a `%` or a backslash comes back as it was (on Unix; on Windows a backslash is a separator).
        #[cfg(not(windows))]
        for name in ["/d/100%.xml", "/d/%7e.xml", "/d/a\\b.xml", "/d/é.xml", "/d/a#b?c.xml"] {
            assert_eq!(path_of(&file_uri(Path::new(name))).as_deref(), Some(name), "{name}");
        }
        assert_eq!(path_of("FILE://localhost/a/b").as_deref(), Some("/a/b"));
        assert_eq!(path_of("file:///C:/dir/x.xml").as_deref(), Some("C:/dir/x.xml"));
        // Only this machine; the query and the fragment are not the path.
        assert_eq!(path_of("file://other-host/a/b"), None);
        assert_eq!(path_of("file:///a/b.xml#frag").as_deref(), Some("/a/b.xml"));
        assert_eq!(path_of("file:///a/b.xml?x=1").as_deref(), Some("/a/b.xml"));
        assert_eq!(path_of("file://localhost").as_deref(), Some(""));
        assert_eq!(path_of("http://x/y"), None);
        assert_eq!(path_of("fi"), None);
        assert_eq!(path_of("é"), None, "no panic on a character that straddles the scheme length");
        assert_eq!(path_of("file:///%zz%4").as_deref(), Some("/%zz%4"), "an escape that is not one stays as it is");
        assert_eq!(path_of("file:///%FF"), None, "bytes that are not text are not a path");
    }
}
