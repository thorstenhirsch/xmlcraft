//! Reading a catalog file: its entries, in document order, with the base each is resolved against and the `prefer` in effect.

use xmlcraft_syntax::{NsScope, SyntaxKind, SyntaxNode, parse, split_qname};

use crate::uri::{normalize_public, normalize_uri, resolve_reference, unwrap_publicid_urn};

/// The namespace of catalog files.
pub const CATALOG_NS: &str = "urn:oasis:names:tc:entity:xmlns:xml:catalog";

/// Entries a catalog may hold: more are ignored (and said so), so that a hostile file cannot make the lists huge.
pub const MAX_ENTRIES: usize = 100_000;
/// The most problems `warnings` lists (the last place says when there were more).
pub const MAX_WARNINGS: usize = 100;
/// How deep `group` elements may nest.
const MAX_DEPTH: usize = 64;

/// One entry of a catalog. Text to compare is already normalised; the places things map to are already absolute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// `prefer` is what the `prefer` in effect said (`public` is true, `system` false), or `None` when no element said: then the
    /// application's choice applies.
    Public {
        id: String,
        uri: String,
        prefer: Option<bool>,
    },
    System {
        id: String,
        uri: String,
    },
    RewriteSystem {
        start: String,
        prefix: String,
    },
    SystemSuffix {
        suffix: String,
        uri: String,
    },
    DelegatePublic {
        start: String,
        catalog: String,
        prefer: Option<bool>,
    },
    DelegateSystem {
        start: String,
        catalog: String,
    },
    Uri {
        name: String,
        uri: String,
    },
    RewriteUri {
        start: String,
        prefix: String,
    },
    UriSuffix {
        suffix: String,
        uri: String,
    },
    DelegateUri {
        start: String,
        catalog: String,
    },
    NextCatalog {
        catalog: String,
    },
}

/// Why a file is not a catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogError {
    /// It is not well-formed XML; the first problem.
    NotWellFormed(String),
    /// The root element is not `catalog` in the catalog namespace.
    NotACatalog,
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::NotWellFormed(why) => write!(f, "the catalog is not well-formed: {why}"),
            CatalogError::NotACatalog => write!(f, "the root element is not a `catalog` in the namespace {CATALOG_NS}"),
        }
    }
}

impl std::error::Error for CatalogError {}

/// A catalog file that has been read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalog {
    /// Where it was read from: what its relative references are resolved against.
    location: String,
    entries: Vec<Entry>,
    /// What was left out and why (an entry that lacks an attribute, one past the limit): nothing is dropped silently.
    warnings: Vec<String>,
}

impl Catalog {
    /// Read the catalog `text`, which was found at `location` (a URI, or `""` when it has none: its relative references then stay
    /// relative).
    pub fn parse(text: &str, location: &str) -> Result<Self, CatalogError> {
        let parsed = parse(text);
        if let Some(first) = parsed.diagnostics().iter().find(|d| d.severity() == xmlcraft_syntax::Severity::Error) {
            return Err(CatalogError::NotWellFormed(format!("at byte {}: {}", first.span.start, first.message())));
        }
        let root = parsed.syntax().children().find(|n| n.kind() == SyntaxKind::Element).ok_or(CatalogError::NotACatalog)?;
        let mut reader = Reader { entries: Vec::new(), warnings: Vec::new(), ns: NsScope::new(), over_limit: false };
        if !reader.element(&root, Context { base: location, prefer: None }, true, 0) {
            return Err(CatalogError::NotACatalog);
        }
        Ok(Self { location: location.to_owned(), entries: reader.entries, warnings: reader.warnings })
    }

    /// Where the catalog was read from.
    pub fn location(&self) -> &str {
        &self.location
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// What was left out of it, and why.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

struct Reader {
    entries: Vec<Entry>,
    warnings: Vec<String>,
    ns: NsScope,
    /// The entry limit was passed (said once).
    over_limit: bool,
}

/// What an element is read in: the base its references are resolved against, and whether public identifiers are preferred.
#[derive(Clone, Copy)]
struct Context<'a> {
    base: &'a str,
    prefer: Option<bool>,
}

/// The attributes of an element: the (prefixed) name, and the value with its references decoded and its white space normalised; and the
/// entity references that are not the five predefined ones (a catalog may declare entities of its own, which this reader does not
/// expand: an element that uses one is left out, not read with the reference as text).
fn attributes(tag: &SyntaxNode) -> (Vec<(String, String)>, Vec<String>) {
    let mut unsupported = Vec::new();
    let attrs = tag
        .children()
        .filter(|n| n.kind() == SyntaxKind::Attribute)
        .filter_map(|attribute| {
            let name = attribute.children_with_tokens().find(|c| c.kind() == SyntaxKind::Name)?.as_token()?.text().to_owned();
            let value = attribute.children().find(|n| n.kind() == SyntaxKind::AttrValue)?;
            let mut text = String::new();
            for token in value.children_with_tokens().filter_map(|c| c.into_token()) {
                match token.kind() {
                    SyntaxKind::AttrText => text.extend(token.text().chars().map(|c| if matches!(c, '\t' | '\r' | '\n') { ' ' } else { c })),
                    SyntaxKind::EntityRef => match token.text() {
                        "&amp;" => text.push('&'),
                        "&lt;" => text.push('<'),
                        "&gt;" => text.push('>'),
                        "&quot;" => text.push('"'),
                        "&apos;" => text.push('\''),
                        other => unsupported.push(other.to_owned()),
                    },
                    SyntaxKind::CharRef => {
                        let reference = token.text();
                        let digits = reference.trim_start_matches("&#").trim_end_matches(';');
                        let code = match digits.strip_prefix(['x', 'X']) {
                            Some(hex) => u32::from_str_radix(hex, 16).ok(),
                            None => digits.parse::<u32>().ok(),
                        };
                        match code.and_then(char::from_u32) {
                            Some(c) => text.push(c),
                            None => text.push_str(reference),
                        }
                    }
                    _ => {}
                }
            }
            Some((name, text))
        })
        .collect();
    (attrs, unsupported)
}

/// The value of the attribute without a prefix called `name`.
fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

impl Reader {
    /// Read `element` and what is in it. Returns whether it is an element of the catalog namespace (the root must be one); others are
    /// skipped with everything in them.
    fn element(&mut self, element: &SyntaxNode, outer: Context<'_>, is_root: bool, depth: usize) -> bool {
        let Some(tag) = element.children().find(|n| n.kind() == SyntaxKind::StartTag) else { return false };
        let Some(name) = tag.children_with_tokens().find(|c| c.kind() == SyntaxKind::Name).and_then(|c| c.as_token().map(|t| t.text().to_owned()))
        else {
            return false;
        };
        let (attrs, unsupported) = attributes(&tag);
        self.ns.push_scope();
        for (attribute, value) in &attrs {
            match split_qname(attribute) {
                Some((None, "xmlns")) => drop(self.ns.declare(None, value)),
                Some((Some("xmlns"), prefix)) => drop(self.ns.declare(Some(prefix), value)),
                _ => {}
            }
        }
        let local = split_qname(&name).and_then(|(prefix, local)| (self.ns.lookup(prefix) == Some(CATALOG_NS)).then(|| local.to_owned()));
        let read = local.is_some();
        if let Some(local) = local.as_ref().filter(|_| !unsupported.is_empty()) {
            self.warn(format!(
                "a `{local}` element uses {} (an entity of the catalog's own, which is not expanded): it and what is in it are left out",
                unsupported.join(", ")
            ));
            self.ns.pop_scope();
            return true;
        }
        if let Some(local) = local {
            // `xml:base` changes what references are resolved against, for this element and what is in it.
            let base = attrs
                .iter()
                .find(|(n, _)| matches!(split_qname(n), Some((Some("xml"), "base"))))
                .map_or_else(|| outer.base.to_owned(), |(_, value)| resolve_reference(outer.base, value.trim()));
            let prefer = match attr(&attrs, "prefer") {
                Some("public") => Some(true),
                Some("system") => Some(false),
                _ => outer.prefer,
            };
            let context = Context { base: &base, prefer };
            match local.as_str() {
                "catalog" if is_root => self.children(element, context, depth),
                "group" if !is_root => self.children(element, context, depth),
                _ if is_root => {
                    self.ns.pop_scope();
                    return false;
                }
                other => self.entry(other, &attrs, context),
            }
        }
        self.ns.pop_scope();
        read
    }

    fn children(&mut self, element: &SyntaxNode, context: Context<'_>, depth: usize) {
        if depth > MAX_DEPTH {
            self.warn(format!("groups are nested more than {MAX_DEPTH} deep: what is inside is left out"));
            return;
        }
        for child in element.children().filter(|n| n.kind() == SyntaxKind::Element) {
            self.element(&child, context, false, depth + 1);
        }
    }

    fn warn(&mut self, message: String) {
        // The list is bounded; its last place says that it was cut, so that no one reads it as complete.
        if self.warnings.len() + 1 < MAX_WARNINGS {
            self.warnings.push(message);
        } else if self.warnings.len() + 1 == MAX_WARNINGS {
            self.warnings.push(format!("more than {} problems: the rest are not listed", MAX_WARNINGS - 1));
        }
    }

    fn entry(&mut self, kind: &str, attrs: &[(String, String)], context: Context<'_>) {
        let needs = |names: &[&str]| -> Option<Vec<String>> { names.iter().map(|n| attr(attrs, n).map(str::to_owned)).collect() };
        let absolute = |reference: &str| resolve_reference(context.base, reference.trim());
        let public = |text: &str| normalize_public(&unwrap_publicid_urn(text).unwrap_or_else(|| text.to_owned()));
        let entry = match kind {
            "public" => needs(&["publicId", "uri"]).map(|v| Entry::Public { id: public(&v[0]), uri: absolute(&v[1]), prefer: context.prefer }),
            "system" => needs(&["systemId", "uri"]).map(|v| Entry::System { id: normalize_uri(&v[0]), uri: absolute(&v[1]) }),
            "rewriteSystem" => needs(&["systemIdStartString", "rewritePrefix"])
                .map(|v| Entry::RewriteSystem { start: normalize_uri(&v[0]), prefix: absolute(&v[1]) }),
            "systemSuffix" => needs(&["systemIdSuffix", "uri"]).map(|v| Entry::SystemSuffix { suffix: normalize_uri(&v[0]), uri: absolute(&v[1]) }),
            "delegatePublic" => needs(&["publicIdStartString", "catalog"]).map(|v| Entry::DelegatePublic {
                start: public(&v[0]),
                catalog: absolute(&v[1]),
                prefer: context.prefer,
            }),
            "delegateSystem" => {
                needs(&["systemIdStartString", "catalog"]).map(|v| Entry::DelegateSystem { start: normalize_uri(&v[0]), catalog: absolute(&v[1]) })
            }
            "uri" => needs(&["name", "uri"]).map(|v| Entry::Uri { name: normalize_uri(&v[0]), uri: absolute(&v[1]) }),
            "rewriteURI" => {
                needs(&["uriStartString", "rewritePrefix"]).map(|v| Entry::RewriteUri { start: normalize_uri(&v[0]), prefix: absolute(&v[1]) })
            }
            "uriSuffix" => needs(&["uriSuffix", "uri"]).map(|v| Entry::UriSuffix { suffix: normalize_uri(&v[0]), uri: absolute(&v[1]) }),
            "delegateURI" => {
                needs(&["uriStartString", "catalog"]).map(|v| Entry::DelegateUri { start: normalize_uri(&v[0]), catalog: absolute(&v[1]) })
            }
            "nextCatalog" => needs(&["catalog"]).map(|v| Entry::NextCatalog { catalog: absolute(&v[0]) }),
            other => {
                self.warn(format!("`{other}` is not an entry of an XML catalog: it is left out"));
                return;
            }
        };
        match entry {
            Some(entry) if self.entries.len() < MAX_ENTRIES => self.entries.push(entry),
            Some(_) => {
                if !self.over_limit {
                    self.over_limit = true;
                    self.warn(format!("more than {MAX_ENTRIES} entries: the rest are left out"));
                }
            }
            None => self.warn(format!("a `{kind}` entry lacks an attribute it needs: it is left out")),
        }
    }
}
