//! The analysis of an XQuery document: its first static error (from the XQuery parser) and an outline of what its prolog
//! declares (functions and variables). It is read again on each change, as the other languages are.

use xmlcraft_syntax::{Code, Diagnostic};
use xmlcraft_xquery::ast::{Decl, Module, written};

use crate::analysis::{Analysis, MAX_OUTLINE_ITEMS, OutlineItem, OutlineKind};
use crate::language::Language;

pub(crate) fn analyze_xquery(text: &str, version: u64) -> Analysis {
    let (diagnostics, outline) = match xmlcraft_xquery::parse::parse(text, &[]) {
        Ok(module) => (Vec::new(), outline(&module, text)),
        Err(e) => {
            let start = e.span.start.min(text.len());
            let end = e.span.end.clamp(start, text.len());
            (vec![Diagnostic::new(Code::XQueryStatic, start..end, vec![e.code.to_owned(), e.message])], Vec::new())
        }
    };
    let outline_truncated = outline.len() >= MAX_OUTLINE_ITEMS;
    Analysis {
        version,
        language: Language::XQuery,
        diagnostics,
        outline,
        outline_truncated,
        folds: Vec::new(),
        folds_truncated: false,
        doctype: None,
        doctype_ids: None,
        schema_hints: Vec::new(),
        root_namespace: None,
        skipped: None,
    }
}

/// The prolog's functions (`name#arity`) and variables (`$name`), each with its declaration as its range.
fn outline(module: &Module, text: &str) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    for decl in &module.prolog {
        let (name, kind) = match &decl.value {
            Decl::Function(f) => (format!("{}#{}", written(&f.name), f.params.len()), OutlineKind::Element),
            Decl::Variable { name, .. } => (format!("${}", written(name)), OutlineKind::Leaf),
            _ => continue,
        };
        if items.len() >= MAX_OUTLINE_ITEMS {
            break;
        }
        let range = decl.span.start.min(text.len())..decl.span.end.min(text.len());
        let preview: String = text.get(range.clone()).unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
        items.push(OutlineItem {
            depth: 0,
            name,
            label: None,
            range: range.clone(),
            name_range: range.start..range.start,
            start_tag: range.clone(),
            end_tag: None,
            end_name_range: None,
            kind,
            children: 0,
            preview,
        });
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_has_its_declarations_as_an_outline_and_its_syntax_error_as_a_problem() {
        let a = analyze_xquery("declare variable $x := 1;\ndeclare function local:f($a) { $a };\nlocal:f($x)", 1);
        assert_eq!(a.diagnostics, Vec::new());
        let names: Vec<&str> = a.outline.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["$x", "local:f#1"]);
        let a = analyze_xquery("for $x in 1 retrun $x", 2);
        assert_eq!(a.diagnostics.len(), 1);
        assert_eq!(a.diagnostics[0].code, Code::XQueryStatic);
        assert!(a.diagnostics[0].message().starts_with("XPST0003: "), "{}", a.diagnostics[0].message());
        assert_eq!(Language::of_name("q.xq"), Language::XQuery);
        assert_eq!(Language::of_name("lib.xqm"), Language::XQuery);
    }
}
