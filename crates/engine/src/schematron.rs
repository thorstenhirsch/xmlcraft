//! Schematron rules named by the document: `<?xml-model href="rules.sch" schematypens="http://purl.oclc.org/dsdl/schematron"?>`. After
//! the DTD or XML Schema check, F8 reads the rules through the resolver (open documents first, then files and the catalogs) and lists
//! what they find in Problems, each at the node that fired it.

use std::cell::RefCell;
use std::ops::Range;

use xmlcraft_resolve::{Budget, Request, Resolver};
use xmlcraft_schematron::{Schema, Severity};
use xmlcraft_syntax::{Code, Diagnostic, SyntaxKind, SyntaxNode, parse};
use xmlcraft_text::encoding::{decode, sniff};

use crate::validation::ValidityProblem;

/// The `schematypens` of Schematron rules.
const SCHEMATRON_TYPE: &str = "http://purl.oclc.org/dsdl/schematron";

/// The rules a document names, and where it names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Named {
    pub href: String,
    pub phase: Option<String>,
    /// The processing instruction.
    pub span: Range<usize>,
}

/// The value of a pseudo-attribute (`name="value"`) in the text of a processing instruction.
fn pseudo(text: &str, name: &str) -> Option<String> {
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        let name_end = rest.find(|c: char| c == '=' || c.is_whitespace())?;
        let (found, after) = rest.split_at_checked(name_end)?;
        let value_part = after.trim_start().strip_prefix('=')?.trim_start();
        let quote = value_part.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let (value, remainder) = value_part.get(1..)?.split_once(quote)?;
        if found == name {
            return Some(value.to_owned());
        }
        rest = remainder;
    }
}

/// The Schematron rules the processing instructions before the root element name (`xml-model` with a `schematypens` of Schematron).
pub fn named(root: &SyntaxNode, text: &str) -> Vec<Named> {
    let mut found = Vec::new();
    for node in root.children() {
        if node.kind() == SyntaxKind::Element {
            break;
        }
        if node.kind() != SyntaxKind::ProcessingInstruction {
            continue;
        }
        let range = usize::from(node.text_range().start())..usize::from(node.text_range().end());
        let Some(pi) = text.get(range.clone()) else { continue };
        let Some(body) = pi.strip_prefix("<?xml-model").filter(|rest| rest.starts_with(char::is_whitespace)) else { continue };
        let body = body.strip_suffix("?>").unwrap_or(body);
        if pseudo(body, "schematypens").as_deref() != Some(SCHEMATRON_TYPE) {
            continue;
        }
        if let Some(href) = pseudo(body, "href").filter(|h| !h.trim().is_empty()) {
            found.push(Named { href, phase: pseudo(body, "phase"), span: range });
        }
    }
    found
}

/// The documents Schematron reads, through the resolver with one budget.
struct Fetch<'r> {
    resolver: &'r Resolver,
    budget: RefCell<Budget>,
}

impl Fetch<'_> {
    fn fetch(&self, reference: &str, base: &str) -> Result<(String, String), String> {
        let found = self.resolver.resolve(&Request::uri(reference, base), &mut self.budget.borrow_mut()).map_err(|f| f.message)?;
        let decoded = decode(&found.bytes, &sniff(&found.bytes));
        if decoded.lossy {
            return Err(format!("{}: it has bytes that are not valid in its encoding", found.location));
        }
        Ok((found.location, decoded.text))
    }
}

/// What the Schematron rules named by the document find in it. `location` is where the document is (relative `href`s are taken from it).
/// Empty when the document names no rules.
pub fn problems(text: &str, location: &str, resolver: &Resolver) -> Vec<ValidityProblem> {
    let parsed = parse(text);
    let root = parsed.syntax();
    let rules = named(&root, text);
    let mut out = Vec::new();
    let own = |code: Code, span: Range<usize>, args: Vec<String>| ValidityProblem { diagnostic: Diagnostic::new(code, span, args), file: None };
    for rule in rules {
        let fetch = Fetch { resolver, budget: RefCell::new(Budget::default()) };
        let (schema_location, schema_text) = match fetch.fetch(&rule.href, location) {
            Ok(found) => found,
            Err(why) => {
                out.push(own(Code::SchematronUnavailable, rule.span.clone(), vec![rule.href.clone(), why]));
                continue;
            }
        };
        let include = |href: &str| fetch.fetch(href, &schema_location).ok().map(|(_, t)| t);
        let schema = match Schema::parse(&schema_text, &include) {
            Ok(schema) => schema,
            Err(e) => {
                out.push(own(Code::SchematronUnavailable, rule.span.clone(), vec![rule.href.clone(), e.message]));
                continue;
            }
        };
        match schema.validate(text, rule.phase.as_deref()) {
            Err(e) => out.push(own(Code::SchematronUnavailable, rule.span.clone(), vec![rule.href.clone(), e.message])),
            Ok(report) => {
                for finding in &report.findings {
                    let code = match finding.severity {
                        Severity::Error => Code::SchematronError,
                        Severity::Warning => Code::SchematronWarning,
                        Severity::Info => Code::SchematronInfo,
                    };
                    let message = match &finding.id {
                        Some(id) => format!("[{id}] {}", finding.message),
                        None => finding.message.clone(),
                    };
                    // (A finding with no place in the text is shown at the rules that were named.)
                    let span = finding.span.clone().map(|s| s.start..s.start.saturating_add(1)).unwrap_or_else(|| rule.span.clone());
                    out.push(own(code, span, vec![message]));
                }
                for why in &report.problems {
                    out.push(own(Code::SchematronProblem, rule.span.clone(), vec![why.clone()]));
                }
                if report.count > report.findings.len() {
                    out.push(own(
                        Code::SchematronInfo,
                        rule.span.clone(),
                        vec![format!("{} more findings are not listed", report.count - report.findings.len())],
                    ));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<(String, Option<String>)> {
        let parsed = parse(text);
        named(&parsed.syntax(), text).into_iter().map(|n| (n.href, n.phase)).collect()
    }

    #[test]
    fn only_xml_model_instructions_for_schematron_before_the_root_name_rules() {
        let t = |extra: &str| format!("<?xml version=\"1.0\"?>\n{extra}\n<r/>");
        assert_eq!(
            names(&t("<?xml-model href=\"a.sch\" schematypens=\"http://purl.oclc.org/dsdl/schematron\" phase=\"p\"?>")),
            [("a.sch".to_owned(), Some("p".to_owned()))]
        );
        assert_eq!(
            names(&t("<?xml-model type='application/xml' schematypens='http://purl.oclc.org/dsdl/schematron' href='b.sch'?>")),
            [("b.sch".to_owned(), None)]
        );
        assert!(names(&t("<?xml-model href=\"a.rng\" schematypens=\"http://relaxng.org/ns/structure/1.0\"?>")).is_empty());
        assert!(names(&t("<?xml-model href=\"a.sch\"?>")).is_empty(), "no schematypens");
        assert!(names(&t("<?xml-stylesheet href=\"a.sch\" schematypens=\"http://purl.oclc.org/dsdl/schematron\"?>")).is_empty());
        assert!(names(&t("<?xml-model schematypens=\"http://purl.oclc.org/dsdl/schematron\"?>")).is_empty(), "no href");
        // After the root element it is not a prolog instruction.
        assert!(names("<r/><?xml-model href=\"a.sch\" schematypens=\"http://purl.oclc.org/dsdl/schematron\"?>").is_empty());
    }
}
