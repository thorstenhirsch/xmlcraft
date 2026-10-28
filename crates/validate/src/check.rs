//! The whole check of a document that has a DOCTYPE: the syntax, the DTD it declares, and the document against it.

use xmlcraft_dtd::{DtdDiagnostic, SubsetLoader, check_document, compile, is_standalone};
use xmlcraft_syntax::{Code, Diagnostic, SyntaxNode};

use crate::validate_document;

/// What is wrong with a document.
pub struct Problems {
    /// Not well-formed, or the DTD or the references to its entities are not right (see `xmlcraft_dtd::problems_of_document`).
    pub well_formedness: Vec<DtdDiagnostic>,
    /// The declarations break a validity constraint, and the document does not match them; a document with no DOCTYPE has nothing to be
    /// valid against. Nothing is said about a document that is not well-formed.
    pub validity: Vec<DtdDiagnostic>,
    /// Where the external texts of the DTD were read from: a problem whose `source` is `n` (other than 0, the document) is in
    /// `sources[n]`.
    pub sources: Vec<String>,
}

/// Check a parsed document: `syntax` are the problems the parser found, `location` is where the document is (for the external parts of
/// its DTD, which `loader` reads).
pub fn check(root: &SyntaxNode, text: &str, syntax: &[Diagnostic], location: &str, loader: &mut dyn SubsetLoader) -> Problems {
    let checked = check_document(root, text, syntax, location, loader);
    let sources = checked.dtd.as_ref().map(|d| d.sources.clone()).unwrap_or_default();
    // The reader finds validity constraints too (the nesting of parameter entities in declarations).
    let (mut validity, well_formedness): (Vec<DtdDiagnostic>, Vec<DtdDiagnostic>) =
        checked.problems.into_iter().partition(|p| p.diagnostic.code.is_validity());
    let well_formed = well_formedness.iter().all(|p| p.diagnostic.severity() != xmlcraft_syntax::Severity::Error);
    if !well_formed {
        return Problems { well_formedness, validity: Vec::new(), sources };
    }
    match &checked.dtd {
        Some(read) => {
            let compiled = compile(&read.dtd, checked.root_name.as_deref());
            validity.extend(compiled.diagnostics);
            validity.extend(
                validate_document(root, &compiled.grammar, is_standalone(root)).into_iter().map(|diagnostic| DtdDiagnostic { source: 0, diagnostic }),
            );
        }
        None => validity.push(DtdDiagnostic { source: 0, diagnostic: Diagnostic::new(Code::VcNoDtd, 0..0, Vec::new()) }),
    }
    validity.sort_by_key(|d| (d.source, d.diagnostic.span.start, d.diagnostic.span.end));
    Problems { well_formedness, validity, sources }
}
