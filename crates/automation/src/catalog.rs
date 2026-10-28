//! `catalog_lookup`: where XML catalogs map an identifier or a URI. The catalogs are files read through the root confinement; the
//! catalogs they name (`nextCatalog`, `delegate…`) are read the same way when they are `file:` URIs, and never fetched otherwise.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_catalog::uri::normalize_uri;
use xmlcraft_catalog::{Catalog, Catalogs, Loader};
use xmlcraft_text::encoding::{decode, sniff};

use crate::file_access::Confinement;
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The largest catalog read.
const MAX_CATALOG_BYTES: usize = 4 * 1024 * 1024;
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

/// A `file:` URI for a path.
fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = if text.starts_with('/') { text } else { format!("/{text}") };
    format!("file://{}", normalize_uri(&text))
}

/// The path a `file:` URI names (percent-escapes decoded), or `None` for any other URI.
fn path_of(uri: &str) -> Option<String> {
    let rest = uri.get(..5).filter(|scheme| scheme.eq_ignore_ascii_case("file:")).map(|_| &uri[5..])?;
    let rest = rest.strip_prefix("//").map_or(rest, |after| after.strip_prefix("localhost").unwrap_or(after));
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        let escaped = (byte == b'%')
            .then(|| bytes.get(i + 1..i + 3).and_then(|hex| std::str::from_utf8(hex).ok()).and_then(|hex| u8::from_str_radix(hex, 16).ok()))
            .flatten();
        match escaped {
            Some(value) => {
                out.push(value);
                i += 3;
            }
            None => {
                out.push(byte);
                i += 1;
            }
        }
    }
    let path = String::from_utf8(out).ok()?;
    // `/C:/dir/file` is a Windows path.
    let windows = path.as_bytes().get(2) == Some(&b':') && path.starts_with('/');
    Some(if windows { path[1..].to_owned() } else { path })
}

/// Reads the catalogs that are named, through the confinement and only when they are files.
struct FileLoader<'a> {
    confinement: &'a Confinement,
    /// How many were read, in all.
    reads: usize,
}

impl FileLoader<'_> {
    fn read(&mut self, path: &str) -> Option<String> {
        let path = self.confinement.resolve_read(path).ok()?;
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
        self.read(&path_of(location)?)
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
    let mut loader = FileLoader { confinement: &a.confinement, reads: 0 };
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
    Ok(vec![Content::Json(json!({ "resolved": resolved, "exists": exists, "catalogs_read": loader.reads, "warnings": warnings }))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uris_and_paths_go_both_ways() {
        assert_eq!(file_uri(Path::new("/etc/xml/cat alog.xml")), "file:///etc/xml/cat%20alog.xml");
        assert_eq!(path_of("file:///etc/xml/cat%20alog.xml").as_deref(), Some("/etc/xml/cat alog.xml"));
        assert_eq!(path_of("FILE://localhost/a/b").as_deref(), Some("/a/b"));
        assert_eq!(path_of("file:///C:/dir/x.xml").as_deref(), Some("C:/dir/x.xml"));
        assert_eq!(path_of("http://x/y"), None);
        assert_eq!(path_of("fi"), None);
        assert_eq!(path_of("é"), None, "no panic on a character that straddles the scheme length");
        assert_eq!(path_of("file:///%zz%4").as_deref(), Some("/%zz%4"), "an escape that is not one stays as it is");
    }
}
