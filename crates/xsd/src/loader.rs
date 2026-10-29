//! Loading the documents of a schema: the root document and everything it includes, imports and redefines.
//!
//! Where a document comes from is the caller's business ([`SchemaLoader`]: open documents, catalogs, files, the cache, the network);
//! this module follows the references, each document once, within limits, and keeps the diagnostics of every document with the number
//! of the document they belong to. It is a loop over a work list, not a recursion: a chain of a thousand includes costs nothing but
//! the limit on documents.

use std::collections::HashMap;

use xmlcraft_syntax::{Code, Diagnostic};

use crate::model::{Item, SchemaDocument};
use crate::reader::read;

/// What kind of reference asks for a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadKind {
    Include,
    Import,
    Redefine,
}

/// A document that is asked for.
#[derive(Clone, Debug)]
pub struct LoadRequest<'a> {
    pub kind: LoadKind,
    /// The namespace an import names (a schema may be found by its namespace when the import has no `schemaLocation`).
    pub namespace: Option<&'a str>,
    /// The `schemaLocation`, as written; relative to `base`.
    pub location: Option<&'a str>,
    /// Where the document that asks was read from.
    pub base: &'a str,
}

/// A document that was found.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub text: String,
    /// Where it was read from (after redirects): what its own relative references are relative to.
    pub location: String,
}

/// Finds the documents a schema refers to.
pub trait SchemaLoader {
    /// The document the request names, or why it cannot be had.
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String>;

    /// What the document found at `location` is: two locations with the same identity are one document, read once (a file asked for as
    /// `Types.xsd` and as `types.xsd` on a file system that folds case; a file and a link to it). The location itself unless the loader
    /// can tell better.
    fn identity(&self, location: &str) -> String {
        location.to_owned()
    }
}

/// A loader that finds nothing: a schema made of one document.
pub struct NoLoader;

impl SchemaLoader for NoLoader {
    fn load(&mut self, _: &LoadRequest<'_>) -> Result<Loaded, String> {
        Err("other documents are not read here".to_owned())
    }
}

/// The most documents one schema may be made of.
pub const MAX_DOCUMENTS: usize = 256;
/// The most text, in all the documents of one schema.
pub const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;

/// A diagnostic and the document it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourcedDiagnostic {
    /// The index into [`Documents::list`].
    pub source: usize,
    pub diagnostic: Diagnostic,
}

/// A document of the schema, as it takes part in it.
#[derive(Clone, Debug)]
pub struct LoadedDocument {
    pub location: String,
    pub document: SchemaDocument,
    /// The namespace its components are in: its target namespace, or, for a document with none that another includes, the includer's
    /// (the "chameleon" include).
    pub namespace: Option<String>,
    /// It has no target namespace and takes that of the document that includes it.
    pub chameleon: bool,
    /// The document that first asked for it (`None` for the root).
    pub asked_by: Option<usize>,
    /// The namespaces this document imports (it may refer to the components of these, besides its own namespace).
    pub imports: Vec<Option<String>>,
    /// The documents it includes or redefines, by index into [`Documents::list`].
    pub includes: Vec<usize>,
}

/// The documents of one schema, and what is wrong with them.
#[derive(Debug, Default)]
pub struct Documents {
    /// The root document first, then the others in the order they were reached.
    pub list: Vec<LoadedDocument>,
    pub diagnostics: Vec<SourcedDiagnostic>,
}

/// A request found in a document, for the work list.
struct Pending {
    from: usize,
    kind: LoadKind,
    namespace: Option<String>,
    location: Option<String>,
    span: std::ops::Range<usize>,
}

/// Read the root document `text` (found at `location`) and everything it refers to.
pub fn load(text: &str, location: &str, loader: &mut dyn SchemaLoader) -> Documents {
    let mut out = Documents::default();
    let mut by_location: HashMap<String, Vec<usize>> = HashMap::new();
    let mut total = text.len();
    let first = read(text, location);
    let Some(root) = first.document else {
        out.diagnostics.extend(first.diagnostics.into_iter().map(|diagnostic| SourcedDiagnostic { source: 0, diagnostic }));
        return out;
    };
    let namespace = root.target_namespace.clone();
    by_location.entry(loader.identity(location)).or_default().push(0);
    out.diagnostics.extend(first.diagnostics.into_iter().map(|diagnostic| SourcedDiagnostic { source: 0, diagnostic }));
    out.list.push(LoadedDocument {
        location: location.to_owned(),
        document: root,
        namespace,
        chameleon: false,
        asked_by: None,
        imports: Vec::new(),
        includes: Vec::new(),
    });
    let mut next = 0;
    while next < out.list.len() {
        let index = next;
        next += 1;
        for request in pending(&out.list[index], index) {
            resolve_one(&mut out, &mut by_location, &mut total, loader, request);
        }
    }
    out.diagnostics.sort_by_key(|d| (d.source, d.diagnostic.span.start, d.diagnostic.span.end));
    out
}

fn pending(doc: &LoadedDocument, from: usize) -> Vec<Pending> {
    let mut found = Vec::new();
    for item in &doc.document.items {
        match item {
            Item::Include(i) => found.push(Pending {
                from,
                kind: LoadKind::Include,
                namespace: None,
                location: Some(i.schema_location.clone()),
                span: i.span.clone(),
            }),
            Item::Import(i) => found.push(Pending {
                from,
                kind: LoadKind::Import,
                namespace: i.namespace.clone(),
                location: i.schema_location.clone(),
                span: i.span.clone(),
            }),
            Item::Redefine(r) => found.push(Pending {
                from,
                kind: LoadKind::Redefine,
                namespace: None,
                location: Some(r.schema_location.clone()),
                span: r.span.clone(),
            }),
            _ => {}
        }
    }
    found
}

fn report(out: &mut Documents, source: usize, code: Code, span: std::ops::Range<usize>, args: &[&str]) {
    out.diagnostics.push(SourcedDiagnostic { source, diagnostic: Diagnostic::new(code, span, args.iter().map(|a| (*a).to_owned()).collect()) });
}

fn resolve_one(
    out: &mut Documents,
    by_location: &mut HashMap<String, Vec<usize>>,
    total: &mut usize,
    loader: &mut dyn SchemaLoader,
    request: Pending,
) {
    let Some(from_doc) = out.list.get(request.from) else { return };
    let base = from_doc.location.clone();
    let from_namespace = from_doc.namespace.clone();
    let from_target = from_doc.document.target_namespace.clone();
    let shown = request.location.clone().or_else(|| request.namespace.clone()).unwrap_or_default();
    if request.kind == LoadKind::Import {
        if request.namespace.is_none() && from_target.is_none() {
            report(out, request.from, Code::XsdImportNeedsNamespace, request.span.clone(), &[]);
        }
        if request.namespace.is_some() && request.namespace == from_target {
            report(out, request.from, Code::XsdImportSameNamespace, request.span.clone(), &[request.namespace.as_deref().unwrap_or_default()]);
            return;
        }
        if let Some(from) = out.list.get_mut(request.from) {
            from.imports.push(request.namespace.clone());
        }
        // An import with neither a location nor a namespace to look for has nothing to load.
        if request.location.is_none() && request.namespace.is_none() {
            return;
        }
    }
    let load_request =
        LoadRequest { kind: request.kind, namespace: request.namespace.as_deref(), location: request.location.as_deref(), base: &base };
    let loaded = match loader.load(&load_request) {
        Ok(loaded) => loaded,
        Err(why) => {
            let code = if request.kind == LoadKind::Import { Code::XsdImportNotLoaded } else { Code::XsdLoadFailed };
            report(out, request.from, code, request.span.clone(), &[&shown, &why]);
            return;
        }
    };
    // A document that is there already is not read again: a document with a target namespace once, one without that is included into
    // a namespace once per namespace.
    let identity = loader.identity(&loaded.location);
    let known = by_location
        .get(&identity)
        .and_then(|list| list.iter().copied().find(|&i| out.list.get(i).is_some_and(|d| !d.chameleon || d.namespace == from_namespace)));
    if let Some(existing) = known {
        let target = out.list.get(existing).and_then(|d| d.document.target_namespace.clone());
        if namespaces_agree(out, &request, &loaded.location, target.as_ref(), from_target.as_ref()) {
            link(out, &request, existing);
        }
        return;
    }
    if out.list.len() >= MAX_DOCUMENTS {
        report(out, request.from, Code::XsdTooManyDocuments, request.span.clone(), &[&format!("at most {MAX_DOCUMENTS} documents are read")]);
        return;
    }
    *total = total.saturating_add(loaded.text.len());
    if *total > MAX_TOTAL_BYTES {
        report(out, request.from, Code::XsdTooManyDocuments, request.span.clone(), &[&format!("at most {MAX_TOTAL_BYTES} bytes are read")]);
        return;
    }
    let result = read(&loaded.text, &loaded.location);
    let Some(document) = result.document else {
        report(out, request.from, Code::XsdNotASchemaDocument, request.span.clone(), &[&loaded.location]);
        return;
    };
    let target = document.target_namespace.clone();
    if !namespaces_agree(out, &request, &loaded.location, target.as_ref(), from_target.as_ref()) {
        return;
    }
    let chameleon = request.kind != LoadKind::Import && target.is_none() && from_namespace.is_some();
    let namespace = if chameleon { from_namespace } else { target };
    let index = out.list.len();
    by_location.entry(identity).or_default().push(index);
    out.diagnostics.extend(result.diagnostics.into_iter().map(|diagnostic| SourcedDiagnostic { source: index, diagnostic }));
    out.list.push(LoadedDocument {
        location: loaded.location,
        document,
        namespace,
        chameleon,
        asked_by: Some(request.from),
        imports: Vec::new(),
        includes: Vec::new(),
    });
    link(out, &request, index);
}

/// An included document is in the namespace of the including one or in none; an imported one is in the namespace the import names.
/// The mistake is reported when they do not agree.
fn namespaces_agree(out: &mut Documents, request: &Pending, location: &str, target: Option<&String>, from_target: Option<&String>) -> bool {
    match request.kind {
        LoadKind::Include | LoadKind::Redefine => {
            if target.is_some() && target != from_target {
                report(out, request.from, Code::XsdIncludeNamespace, request.span.clone(), &[location, target.map_or("", String::as_str)]);
                return false;
            }
        }
        LoadKind::Import => {
            if target != request.namespace.as_ref() {
                report(
                    out,
                    request.from,
                    Code::XsdImportNamespace,
                    request.span.clone(),
                    &[location, request.namespace.as_deref().unwrap_or("(none)")],
                );
                return false;
            }
        }
    }
    true
}

/// Record that the document `from` includes (or redefines) `to`.
fn link(out: &mut Documents, request: &Pending, to: usize) {
    if request.kind != LoadKind::Import
        && let Some(from) = out.list.get_mut(request.from)
        && !from.includes.contains(&to)
    {
        from.includes.push(to);
    }
}
