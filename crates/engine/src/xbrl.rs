//! XBRL instances in the validator: after the DTD or schema check, F8 lists what the checks of `xmlcraft-xbrl` find in an XBRL 2.1
//! instance or an Inline XBRL document, each at its fact or context.

use std::ops::Range;

use xmlcraft_resolve::{Budget, Request, Resolver};
use xmlcraft_syntax::{Code, Diagnostic};
use xmlcraft_text::encoding::{decode, sniff};
use xmlcraft_xbrl::taxonomy::{Loader, check_taxonomy, load};
use xmlcraft_xbrl::{Level, check, is_xbrl, read};

use crate::validation::ValidityProblem;

/// Whether `text` is an XBRL instance or an Inline XBRL document that can be read.
pub fn recognised(text: &str) -> bool {
    is_xbrl(text) && read(text).is_ok()
}

/// The schema documents of a taxonomy, found through the resolver (the open documents, the files, the catalogs and the bundle first, the
/// web where the app may), one budget for the whole taxonomy.
struct TaxonomyDocuments<'r> {
    resolver: &'r Resolver,
    budget: Budget,
}

impl Loader for TaxonomyDocuments<'_> {
    fn load(&mut self, location: &str, base: &str) -> Result<(String, String), String> {
        let found = self.resolver.resolve(&Request::uri(location, base), &mut self.budget).map_err(|f| f.message)?;
        let decoded = decode(&found.bytes, &sniff(&found.bytes));
        if decoded.lossy {
            return Err("it has bytes that are not valid in its encoding".to_owned());
        }
        Ok((found.location, decoded.text))
    }
}

/// What was read of the taxonomy an instance names, in numbers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaxonomySummary {
    /// The schema documents and linkbases read.
    pub documents: usize,
    /// Those that could not be read.
    pub not_loaded: usize,
    pub concepts: usize,
    /// The summation-item relationships (by parent and role).
    pub calculations: usize,
    pub hypercubes: usize,
    pub dimensions: usize,
    pub typed_dimensions: usize,
    /// The distinct members of the dimension domains.
    pub members: usize,
    /// The concepts that have hypercubes (their own or inherited).
    pub primary_items: usize,
}

/// What the XBRL tool window and `xbrl_facts` show of a document: the instance read, every finding (of the instance on its own, of the
/// taxonomy, of the calculations and of the dimensions), and what was read of the taxonomy (none when the instance names none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub instance: xmlcraft_xbrl::Instance,
    pub findings: Vec<xmlcraft_xbrl::Finding>,
    pub taxonomy: Option<TaxonomySummary>,
}

/// What `instance` (at `location`) says wrong of itself and, from the taxonomy its `schemaRef`s name (read through `resolver`), what the
/// concepts say of its facts.
pub fn findings(instance: &xmlcraft_xbrl::Instance, location: &str, resolver: &Resolver) -> Vec<xmlcraft_xbrl::Finding> {
    checked(instance, location, resolver).0
}

/// [`findings`], and what was read of the taxonomy.
fn checked(instance: &xmlcraft_xbrl::Instance, location: &str, resolver: &Resolver) -> (Vec<xmlcraft_xbrl::Finding>, Option<TaxonomySummary>) {
    let mut found = check(instance);
    if instance.schema_refs.is_empty() {
        return (found, None);
    }
    let taxonomy = load(&instance.schema_refs, location, &mut TaxonomyDocuments { resolver, budget: Budget::default() });
    found.extend(check_taxonomy(instance, &taxonomy));
    let d = &taxonomy.dimensions;
    let summary = TaxonomySummary {
        documents: taxonomy.documents.len(),
        not_loaded: taxonomy.not_loaded.len(),
        concepts: taxonomy.concepts.len(),
        calculations: taxonomy.calculations.len(),
        hypercubes: d.hypercubes.len(),
        dimensions: d.dimensions.len(),
        typed_dimensions: d.dimensions.values().filter(|t| t.is_some()).count(),
        members: d.member_count(),
        primary_items: d.primary_items.len(),
    };
    (found, Some(summary))
}

/// Read `text` (an XBRL instance or an Inline XBRL document at `location`) and check it as F8 does, with the taxonomy its `schemaRef`s
/// name read through `resolver`. The findings are in the order of the text (those about the whole document first).
pub fn report(text: &str, location: &str, resolver: &Resolver) -> Result<Report, String> {
    let instance = read(text).map_err(|e| e.to_string())?;
    let (mut findings, taxonomy) = checked(&instance, location, resolver);
    findings.sort_by_key(|f| (f.span.start, f.span.end));
    Ok(Report { instance, findings, taxonomy })
}

/// What the XBRL checks find in `text` (an instance at `location`): what it says of itself, and, from the taxonomy its `schemaRef`s name
/// (read through `resolver`), what the concepts say of its facts. Nothing for text that is not an XBRL document or cannot be read.
pub fn problems(text: &str, location: &str, resolver: &Resolver) -> Vec<ValidityProblem> {
    if !is_xbrl(text) {
        return Vec::new();
    }
    let Ok(instance) = read(text) else { return Vec::new() };
    findings(&instance, location, resolver)
        .into_iter()
        .map(|f| {
            let code = match f.level {
                Level::Error => Code::XbrlError,
                Level::Warning => Code::XbrlWarning,
            };
            // A finding about the whole document (no schemaRef) is shown at the start of the root element.
            let span: Range<usize> = if f.span.is_empty() { 0..1 } else { f.span.start..f.span.start.saturating_add(1) };
            ValidityProblem { diagnostic: Diagnostic::new(code, span, vec![f.message]), file: None }
        })
        .collect()
}
