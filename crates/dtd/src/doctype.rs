//! What the DOCTYPE of a document says, read from its syntax tree: the name, the external identifier and where the internal subset is.

use std::ops::Range;

use xmlcraft_syntax::{Diagnostic, SyntaxKind, SyntaxNode};

use crate::compile::{Compiled, compile};
use crate::namespaces::{check_namespaces, namespaces_need_grammar};
use crate::parse::{DoctypeInput, DtdDiagnostic, ExternalRef, NoLoader, Parsed, SubsetLoader, parse_doctype};
use crate::references::check_references;

/// The parts of a DOCTYPE declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoctypeParts {
    /// The name of the root element it names.
    pub name: Option<String>,
    /// `PUBLIC "public" "system"`, `SYSTEM "system"`: the identifiers as written.
    pub public: Option<String>,
    pub system: Option<String>,
    /// The text between `[` and `]` (to the end of the declaration when there is no `]`).
    pub subset: Option<Range<usize>>,
    /// The whole declaration.
    pub range: Range<usize>,
}

fn range_of(node: &SyntaxNode) -> Range<usize> {
    usize::from(node.text_range().start())..usize::from(node.text_range().end())
}

/// The first DOCTYPE of the document `root`, if it has one.
pub fn doctype_parts(root: &SyntaxNode) -> Option<DoctypeParts> {
    let doctype = root.children().find(|n| n.kind() == SyntaxKind::Doctype)?;
    let mut parts = DoctypeParts { name: None, public: None, system: None, subset: None, range: range_of(&doctype) };
    let mut keyword: Option<String> = None;
    let mut literals: Vec<String> = Vec::new();
    let mut open_quote: Option<String> = None;
    for element in doctype.children_with_tokens() {
        if let Some(node) = element.as_node() {
            if node.kind() == SyntaxKind::DoctypeSubset {
                let range = range_of(node);
                let mut inner = range.clone();
                for token in node.children_with_tokens().filter_map(|c| c.into_token()) {
                    match token.kind() {
                        SyntaxKind::LBracket if inner.start == range.start => inner.start = usize::from(token.text_range().end()),
                        SyntaxKind::RBracket => inner.end = usize::from(token.text_range().start()),
                        _ => {}
                    }
                }
                parts.subset = Some(inner.start..inner.end.max(inner.start));
            }
            continue;
        }
        let Some(token) = element.as_token() else { continue };
        match token.kind() {
            SyntaxKind::Name if parts.name.is_none() => parts.name = Some(token.text().to_owned()),
            SyntaxKind::Name if keyword.is_none() && matches!(token.text(), "SYSTEM" | "PUBLIC") => keyword = Some(token.text().to_owned()),
            // A literal is what is between two quotes, which may be nothing.
            SyntaxKind::Quote => match open_quote.take() {
                Some(text) => literals.push(text),
                None => open_quote = Some(String::new()),
            },
            SyntaxKind::LiteralText => {
                if let Some(text) = open_quote.as_mut() {
                    text.push_str(token.text());
                }
            }
            _ => {}
        }
    }
    let mut literals = literals.into_iter();
    match keyword.as_deref() {
        Some("SYSTEM") => parts.system = literals.next(),
        Some("PUBLIC") => {
            parts.public = literals.next();
            parts.system = literals.next();
        }
        _ => {}
    }
    Some(parts)
}

/// Every problem in the DTD of a document and in its references to entities: what the syntax tree found (`syntax`) except inside the
/// internal subset, whose declarations this crate reads and reports better, what reading the DTD found, and what the references to
/// general entities break (declared, parsed, not external in an attribute value, not recursive, well-formed). `loader` is where the
/// external parts come from. Sorted by where they are (those of the document first).
pub fn problems_of_document(
    root: &SyntaxNode,
    text: &str,
    syntax: &[Diagnostic],
    location: &str,
    loader: &mut dyn SubsetLoader,
) -> Vec<DtdDiagnostic> {
    check_document(root, text, syntax, location, loader).problems
}

/// What [`check_document`] found.
pub struct Checked {
    /// The problems, sorted by where they are (those of the document first): see [`problems_of_document`].
    pub problems: Vec<DtdDiagnostic>,
    /// The DTD that was read, when the document has a DOCTYPE.
    pub dtd: Option<Parsed>,
    /// The name the DOCTYPE gives the root element.
    pub root_name: Option<String>,
    /// The DTD compiled, when finding the problems needed it (the namespace rules do, for a DTD with entities or `xmlns` attributes): a
    /// caller that compiles it anyway takes it from here instead of doing the work twice. It is `None` for most documents.
    pub compiled: Option<Compiled>,
}

/// [`problems_of_document`], and the DTD that was read to find them (so that it need not be read again).
pub fn check_document(root: &SyntaxNode, text: &str, syntax: &[Diagnostic], location: &str, loader: &mut dyn SubsetLoader) -> Checked {
    let parts = doctype_parts(root);
    let inside = parts.as_ref().and_then(|p| p.subset.clone());
    let mut all: Vec<DtdDiagnostic> = syntax
        .iter()
        .filter(|d| !inside.as_ref().is_some_and(|range| d.span.start >= range.start && d.span.end <= range.end))
        .map(|d| DtdDiagnostic { source: 0, diagnostic: d.clone() })
        .collect();
    let mut dtd = None;
    let mut compiled = None;
    if let Some(parts) = &parts {
        let internal = parts.subset.clone().and_then(|range| text.get(range.clone()).map(|subset| (subset, range.start)));
        let external = (parts.system.is_some() || parts.public.is_some())
            .then_some(ExternalRef { public: parts.public.as_deref(), system: parts.system.as_deref() });
        let mut read = parse_doctype(&DoctypeInput { external, internal, location }, loader);
        all.extend(check_references(root, &read.dtd, location, loader));
        if namespaces_need_grammar(&read.dtd) {
            let grammar = compiled.insert(compile(&read.dtd, parts.name.as_deref()));
            all.extend(check_namespaces(root, &grammar.grammar));
        }
        all.append(&mut read.diagnostics);
        dtd = Some(read);
    }
    all.sort_by_key(|d| (d.source, d.diagnostic.span.start, d.diagnostic.span.end));
    Checked { problems: all, dtd, root_name: parts.and_then(|p| p.name), compiled }
}

/// [`problems_of_document`] with no external part read: what is wrong in the document itself (its own text, its internal subset).
pub fn problems_in_text(root: &SyntaxNode, text: &str, syntax: &[Diagnostic]) -> Vec<Diagnostic> {
    use xmlcraft_syntax::Code;
    problems_of_document(root, text, syntax, "", &mut NoLoader)
        .into_iter()
        // What is not read is not a problem of the text, and the validity constraints are for validation to say.
        .filter(|d| {
            d.source == 0 && !matches!(d.diagnostic.code, Code::DtdExternalNotRead | Code::DtdUndeclaredPeUnread) && !d.diagnostic.code.is_validity()
        })
        .map(|d| d.diagnostic)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xmlcraft_syntax::parse;

    fn parts(text: &str) -> Option<DoctypeParts> {
        doctype_parts(&parse(text).syntax())
    }

    #[test]
    fn the_dtd_is_compiled_for_the_problems_only_when_the_namespace_rules_need_it() {
        let checked = |text: &str| {
            let parsed = parse(text);
            check_document(&parsed.syntax(), text, parsed.diagnostics(), "", &mut NoLoader)
        };
        assert!(checked("<!DOCTYPE r [<!ELEMENT r ANY>]><r/>").compiled.is_none(), "nothing to read twice: no grammar is made");
        assert!(checked("<r/>").compiled.is_none());
        assert!(checked("<!DOCTYPE r [<!ELEMENT r ANY><!ENTITY e 'x'>]><r/>").compiled.is_some(), "an entity can change what a declaration says");
        assert!(checked("<!DOCTYPE r [<!ELEMENT r ANY><!ATTLIST r xmlns CDATA #IMPLIED>]><r/>").compiled.is_some());
        assert!(checked("<!DOCTYPE r [<!ELEMENT r ANY><!ATTLIST r xmlns:a CDATA #IMPLIED>]><r/>").compiled.is_some());
    }

    #[test]
    fn the_name_the_identifiers_and_the_subset_are_found() {
        let text = "<!DOCTYPE r PUBLIC \"-//X//EN\" 'x.dtd' [ <!ELEMENT r ANY> ]><r/>";
        let p = parts(text).expect("a doctype");
        assert_eq!((p.name.as_deref(), p.public.as_deref(), p.system.as_deref()), (Some("r"), Some("-//X//EN"), Some("x.dtd")));
        assert_eq!(p.subset.clone().map(|r| &text[r]), Some(" <!ELEMENT r ANY> "));
        assert_eq!(&text[p.range], "<!DOCTYPE r PUBLIC \"-//X//EN\" 'x.dtd' [ <!ELEMENT r ANY> ]>");
        let p = parts("<!DOCTYPE r SYSTEM \"\"><r/>").expect("a doctype");
        assert_eq!((p.system.as_deref(), p.public), (Some(""), None), "an empty literal is a literal");
        let p = parts("<!DOCTYPE r [<!ENTITY a 'b'>]><r/>").expect("a doctype");
        assert!(p.system.is_none() && p.subset.is_some());
        let p = parts("<!DOCTYPE r><r/>").expect("a doctype");
        assert_eq!((p.name.as_deref(), p.system, p.subset), (Some("r"), None, None));
        assert_eq!(parts("<r/>"), None);
    }

    #[test]
    fn a_doctype_that_is_not_finished_does_not_panic() {
        for text in [
            "<!DOCTYPE",
            "<!DOCTYPE r [",
            "<!DOCTYPE r [ <!ELEMENT",
            "<!DOCTYPE r PUBLIC '",
            "<!DOCTYPE r PUBLIC 'a' 'b' [ ]]",
            "<!DOCTYPE [",
            "<!DOCTYPE r SYSTEM [ ]>",
        ] {
            let _ = parts(text);
        }
        let p = parts("<!DOCTYPE r [ x").expect("a doctype");
        assert!(p.subset.is_some());
    }
}
