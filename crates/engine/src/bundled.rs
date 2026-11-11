//! The XML schemas and DTDs that ship with XmlCraft, as a list to browse and assign.
//!
//! The files themselves are packed into the binary by `xmlcraft-schemas`. This is the view of them a person or an agent chooses from:
//! the paths, the target namespace of an XML Schema, and the public location to write into a document. A document names its schema
//! by the location its publisher gives it (the W3C or OASIS address), never by the app's private `bundled:/` URI: the bundled catalog
//! maps that address back to the file that ships, so the document validates offline here and anywhere else as it would online. A file
//! the catalog knows only by a namespace (UBL) has no such address and is not offered for assigning.

use std::sync::OnceLock;

use xmlcraft_catalog::{Catalog, Entry};
use xmlcraft_schemas as bundle;

/// What a bundled file is read as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundledFormat {
    /// An XML Schema.
    Xsd,
    /// A DTD.
    Dtd,
}

impl BundledFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Xsd => "xsd",
            Self::Dtd => "dtd",
        }
    }

    fn of(path: &str) -> Option<Self> {
        let (_, ext) = path.rsplit_once('.')?;
        if ext.eq_ignore_ascii_case("xsd") {
            Some(Self::Xsd)
        } else if ext.eq_ignore_ascii_case("dtd") {
            Some(Self::Dtd)
        } else {
            None
        }
    }
}

/// One XML Schema or DTD in the bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundledSchema {
    /// The path in the bundle, such as `w3c/xmlschema/XMLSchema.xsd`.
    pub path: &'static str,
    /// The location to write into a document, such as `bundled:/w3c/xmlschema/XMLSchema.xsd`.
    pub uri: String,
    /// The last part of the path.
    pub name: &'static str,
    /// The set the file belongs to, such as `w3c` or `ubl`.
    pub set: &'static str,
    pub format: BundledFormat,
    /// The address to write into a document (`SYSTEM` for a DTD, the schema location for an XML Schema), when the catalog maps one.
    pub location: Option<String>,
    /// For a DTD, the public identifier the catalog maps to it, when there is one.
    pub public_id: Option<String>,
}

/// Every XML Schema and DTD in the bundle, in path order. The licence texts, catalogs, entity sets and modules are not schemas to
/// assign, so they are left out.
pub fn list() -> Vec<BundledSchema> {
    bundle::paths()
        .filter_map(|path| {
            let format = BundledFormat::of(path)?;
            let name = path.rsplit('/').next().unwrap_or(path);
            let set = path.split_once('/').map_or("", |(set, _)| set);
            let uri = bundle::uri_of(path);
            let location = public_location(&uri);
            let public_id = if format == BundledFormat::Dtd { public_id_of_bundled(&uri) } else { None };
            Some(BundledSchema { path, uri, name, set, format, location, public_id })
        })
        .collect()
}

/// The schemas whose path, file name or set contains `query` (without case). Everything when it is empty.
pub fn matching(query: &str) -> Vec<BundledSchema> {
    let query = query.trim().to_ascii_lowercase();
    list()
        .into_iter()
        .filter(|schema| {
            query.is_empty()
                || schema.path.to_ascii_lowercase().contains(&query)
                || schema.name.to_ascii_lowercase().contains(&query)
                || schema.set.to_ascii_lowercase().contains(&query)
        })
        .collect()
}

/// The target namespace of a bundled XML Schema, named by its `bundled:/` URI or by the public location the catalog maps to it.
pub fn namespace_of(location: &str) -> Option<String> {
    let uri = if bundle::path_of(location).is_some() { location.to_owned() } else { bundled_for_system(location)? };
    let path = bundle::path_of(&uri)?;
    crate::assign::schema_namespace(&bundle::get_text(path)?)
}

/// The location to write into a document for `location`: a `bundled:/` URI becomes the public address the catalog maps to that file;
/// anything else stays as it is. A bundled file with no public address cannot be named (the document would only work in this app).
pub fn location_to_write(location: &str) -> Result<String, String> {
    if location.starts_with(bundle::URI_PREFIX) {
        return public_location(location)
            .ok_or_else(|| format!("{location} has no public address to name it by (a document of its namespace finds it without naming it)"));
    }
    Ok(location.to_owned())
}

/// The public identifier of the DTD whose system identifier is `system`, when the bundled catalog maps both to the same file: what a
/// DOCTYPE that names that DTD says besides the address.
pub fn public_id_of(system: &str) -> Option<String> {
    public_id_of_bundled(&bundled_for_system(system)?)
}

/// The entries of the bundled catalogs, the root catalog first and those it names with `nextCatalog` after it, in order.
fn entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut out = Vec::new();
        let mut queue = vec![bundle::uri_of(bundle::ROOT_CATALOG)];
        let mut seen = Vec::new();
        // (The catalogs are the app's own files; the bound is for a mistake in them, not for an attack.)
        while let Some(uri) = queue.pop() {
            if seen.contains(&uri) || seen.len() >= 16 {
                continue;
            }
            seen.push(uri.clone());
            let Some(text) = bundle::path_of(&uri).and_then(bundle::get_text) else { continue };
            let Ok(catalog) = Catalog::parse(&text, &uri) else { continue };
            let mut next = Vec::new();
            for entry in catalog.entries() {
                if let Entry::NextCatalog { catalog } = entry {
                    next.push(catalog.clone());
                }
                out.push(entry.clone());
            }
            // (Later catalogs are read in the order they are named.)
            queue.extend(next.into_iter().rev());
        }
        out
    })
}

/// The address the bundled file `uri` is published at: the first `system` or `uri` entry that maps a location ending in the file's
/// name to it (a `uri` entry that maps a namespace is not a location), or the first `rewriteSystem` that covers it.
fn public_location(uri: &str) -> Option<String> {
    let file = uri.rsplit('/').next()?;
    let names_a_file = |id: &str| id.rsplit('/').next() == Some(file) && id.contains("://");
    entries().iter().find_map(|entry| match entry {
        Entry::System { id, uri: to } | Entry::Uri { name: id, uri: to } if to == uri && names_a_file(id) => Some(id.clone()),
        Entry::RewriteSystem { start, prefix } => uri.strip_prefix(prefix.as_str()).map(|rest| format!("{start}{rest}")),
        _ => None,
    })
}

/// The public identifier the catalog maps to the bundled file `uri`.
fn public_id_of_bundled(uri: &str) -> Option<String> {
    entries().iter().find_map(|entry| match entry {
        Entry::Public { id, uri: to, .. } if to == uri => Some(id.clone()),
        _ => None,
    })
}

/// The bundled file the address `system` stands for in the catalog (a `system`, `uri` or `rewriteSystem` entry).
fn bundled_for_system(system: &str) -> Option<String> {
    entries()
        .iter()
        .find_map(|entry| match entry {
            Entry::System { id, uri } | Entry::Uri { name: id, uri } if id == system => Some(uri.clone()),
            Entry::RewriteSystem { start, prefix } => system.strip_prefix(start.as_str()).map(|rest| format!("{prefix}{rest}")),
            _ => None,
        })
        .filter(|uri| bundle::path_of(uri).is_some_and(bundle::contains))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schemas_people_recognise_are_listed_with_the_location_and_namespace_to_use() {
        let list = list();
        let xml_schema = list.iter().find(|s| s.path == "w3c/xmlschema/XMLSchema.xsd").expect("the schema for schemas");
        assert_eq!(xml_schema.name, "XMLSchema.xsd");
        assert_eq!(xml_schema.set, "w3c");
        assert_eq!(xml_schema.format, BundledFormat::Xsd);
        assert_eq!(xml_schema.uri, "bundled:/w3c/xmlschema/XMLSchema.xsd");
        assert_eq!(namespace_of(&xml_schema.uri).as_deref(), Some(xmlcraft_xsd::XSD_NAMESPACE));
        let xhtml = list.iter().find(|s| s.path == "w3c/xhtml1/xhtml1-strict.dtd").expect("the XHTML DTD");
        assert_eq!(xhtml.format, BundledFormat::Dtd);
        assert_eq!(namespace_of(&xhtml.uri), None, "a DTD has no target namespace");
        assert_eq!(namespace_of("bundled:/nope.xsd"), None);
        assert_eq!(namespace_of("file:/tmp/a.xsd"), None);
    }

    #[test]
    fn a_bundled_schema_is_assigned_by_the_address_its_publisher_gives_it() {
        let list = list();
        let xml_schema = list.iter().find(|s| s.path == "w3c/xmlschema/XMLSchema.xsd").expect("the schema for schemas");
        assert_eq!(xml_schema.location.as_deref(), Some("http://www.w3.org/2001/XMLSchema.xsd"), "not the namespace, not bundled:/");
        assert_eq!(namespace_of("http://www.w3.org/2001/XMLSchema.xsd").as_deref(), Some(xmlcraft_xsd::XSD_NAMESPACE));
        let strict = list.iter().find(|s| s.path == "w3c/xhtml1/xhtml1-strict.dtd").expect("the XHTML DTD");
        assert_eq!(strict.location.as_deref(), Some("http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"));
        assert_eq!(strict.public_id.as_deref(), Some("-//W3C//DTD XHTML 1.0 Strict//EN"));
        assert_eq!(public_id_of("http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd").as_deref(), Some("-//W3C//DTD XHTML 1.0 Strict//EN"));
        assert_eq!(public_id_of("http://example.com/a.dtd"), None);
        let invoice = list.iter().find(|s| s.path == "ubl/maindoc/UBL-Invoice-2.1.xsd").expect("UBL");
        assert_eq!(invoice.location, None, "known only by its namespace: nothing to write");
        assert!(list.iter().filter_map(|s| s.location.as_deref()).all(|l| !l.starts_with("bundled:")));
        assert_eq!(location_to_write(&strict.uri).as_deref(), Ok("http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"));
        assert!(location_to_write(&invoice.uri).is_err());
        assert_eq!(location_to_write("order.xsd").as_deref(), Ok("order.xsd"));
    }

    #[test]
    fn only_schemas_and_dtds_are_offered_and_each_one_once() {
        let list = list();
        assert!(list.len() >= 30, "the bundle has the full sets: {}", list.len());
        assert!(list.iter().all(|s| s.format == BundledFormat::of(s.path).expect("a schema or a DTD")));
        assert!(list.iter().all(|s| !s.path.ends_with(".ent") && !s.path.ends_with(".mod") && !s.path.ends_with(".json")));
        let uris: std::collections::HashSet<&str> = list.iter().map(|s| s.uri.as_str()).collect();
        assert_eq!(uris.len(), list.len(), "a path is listed once");
        assert!(uris.contains("bundled:/ubl/maindoc/UBL-Invoice-2.1.xsd"));
    }

    #[test]
    fn a_query_finds_by_path_name_or_set_case_insensitively() {
        let found = matching("XHTML1-STRICT");
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].name, "xhtml1-strict.dtd");
        assert!(matching("ubl").iter().all(|s| s.set == "ubl" || s.path.to_ascii_lowercase().contains("ubl")));
        assert!(matching("ubl").len() >= 10);
        assert_eq!(matching("  XMLSchema.xsd ").iter().filter(|s| s.name == "XMLSchema.xsd").count(), 1);
        assert_eq!(matching("").len(), list().len());
        assert!(matching("what no schema is called").is_empty());
    }
}
