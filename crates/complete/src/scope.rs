//! The namespace declarations in scope at a place in a document, and how a name is written there.

use xmlcraft_syntax::{SyntaxKind, SyntaxNode, decode_references};
use xmlcraft_xsd::model::QName;

pub(crate) const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
pub(crate) const XSI_NAMESPACE: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// A namespace declaration that a new name needs: the prefix (`None`: the default namespace) and the namespace name (empty for
/// `xmlns=""`, which undeclares the default namespace).
pub type Declaration = (Option<String>, String);

/// A name as it is written at a place, and the declaration it needs there, if it is not in scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    pub name: String,
    pub declare: Option<Declaration>,
}

/// The declarations in scope, the innermost last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Scope(Vec<Declaration>);

/// The attributes of a start tag, as (name, value) with the value unquoted and its references decoded.
pub(crate) fn attributes_of(start_tag: &SyntaxNode) -> Vec<(String, String)> {
    start_tag
        .children()
        .filter(|c| c.kind() == SyntaxKind::Attribute)
        .filter_map(|attribute| {
            let name = attribute.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name)?;
            let value = attribute.children().find(|c| c.kind() == SyntaxKind::AttrValue)?.text().to_string();
            let inner = match (value.chars().next(), value.chars().last()) {
                (Some(open @ ('"' | '\'')), Some(close)) if open == close && value.len() >= 2 => value.get(1..value.len() - 1).unwrap_or_default(),
                _ => value.as_str(),
            };
            Some((name.text().to_owned(), decode_references(inner)))
        })
        .collect()
}

impl Scope {
    /// The declarations made in a start tag come into scope.
    pub(crate) fn declare_in(&mut self, start_tag: &SyntaxNode) {
        for (name, value) in attributes_of(start_tag) {
            if name == "xmlns" {
                self.0.push((None, value));
            } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                self.0.push((Some(prefix.to_owned()), value));
            }
        }
    }

    /// The scope that has these declarations (the innermost last).
    pub(crate) fn from_declarations(declarations: &[Declaration]) -> Scope {
        Scope(declarations.to_vec())
    }

    /// The declarations, the innermost last.
    pub(crate) fn declarations(&self) -> &[Declaration] {
        &self.0
    }

    /// The scope inside an element that makes `declaration`.
    pub(crate) fn with(&self, declaration: Option<&Declaration>) -> Scope {
        let mut scope = self.clone();
        scope.0.extend(declaration.cloned());
        scope
    }

    /// The namespace a prefix (none: the default namespace) stands for here. An undeclared one, and an undeclared default
    /// namespace, stand for none.
    pub(crate) fn resolve(&self, prefix: Option<&str>) -> Option<&str> {
        if prefix == Some("xml") {
            return Some(XML_NAMESPACE);
        }
        let found = self.0.iter().rev().find(|(p, _)| p.as_deref() == prefix).map(|(_, uri)| uri.as_str());
        found.filter(|uri| !uri.is_empty())
    }

    /// The expanded name of a name as it is written: an unprefixed element is in the default namespace, an unprefixed attribute in none.
    /// A prefix that is not declared leaves the name as written, in no namespace.
    pub(crate) fn expand(&self, written: &str, default_namespace: bool) -> QName {
        match written.split_once(':') {
            Some((prefix, local)) => match self.resolve(Some(prefix)) {
                Some(namespace) => QName { namespace: Some(namespace.to_owned()), local: local.to_owned() },
                None => QName { namespace: None, local: written.to_owned() },
            },
            None => QName { namespace: if default_namespace { self.resolve(None).map(str::to_owned) } else { None }, local: written.to_owned() },
        }
    }

    /// A prefix that is not in use here.
    fn new_prefix(&self) -> String {
        (1usize..)
            .map(|n| format!("ns{n}"))
            .find(|candidate| !self.0.iter().any(|(p, _)| p.as_deref() == Some(candidate.as_str())))
            .unwrap_or_else(|| "ns".to_owned())
    }

    /// How to write the name `local` in `namespace` as an element (or, with `attribute`, as an attribute) here: with the prefix the
    /// namespace has in scope, or without one when it is the default namespace; when it is in none of them, with the declaration that
    /// puts it there (the default namespace for an element, if that is free, else a prefix). An element in no namespace where a default
    /// namespace is in scope needs `xmlns=""`.
    pub(crate) fn write(&self, namespace: Option<&str>, local: &str, attribute: bool) -> Written {
        let plain = |declare| Written { name: local.to_owned(), declare };
        let default = self.resolve(None);
        let Some(namespace) = namespace else {
            // No namespace: an attribute is never in the default one; an element is, unless the default is undeclared.
            return if attribute || default.is_none() { plain(None) } else { plain(Some((None, String::new()))) };
        };
        if namespace == XML_NAMESPACE {
            return Written { name: format!("xml:{local}"), declare: None };
        }
        if !attribute && default == Some(namespace) {
            return plain(None);
        }
        // A prefix that stands for the namespace here (the innermost declaration of the prefix is the one that counts).
        let bound = self.0.iter().rev().find_map(|(prefix, uri)| {
            let prefix = prefix.as_deref()?;
            (uri == namespace && self.resolve(Some(prefix)) == Some(namespace)).then_some(prefix)
        });
        if let Some(prefix) = bound {
            return Written { name: format!("{prefix}:{local}"), declare: None };
        }
        if !attribute && default.is_none() {
            return plain(Some((None, namespace.to_owned())));
        }
        let prefix = self.new_prefix();
        Written { name: format!("{prefix}:{local}"), declare: Some((Some(prefix), namespace.to_owned())) }
    }
}

/// ` xmlns:p="uri"` (or ` xmlns="uri"`) for a declaration, to put in a start tag.
pub(crate) fn declaration_text(declaration: &Declaration) -> String {
    let uri = declaration.1.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;");
    match &declaration.0 {
        Some(prefix) => format!(" xmlns:{prefix}=\"{uri}\""),
        None => format!(" xmlns=\"{uri}\""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(declarations: &[(Option<&str>, &str)]) -> Scope {
        Scope(declarations.iter().map(|(p, u)| (p.map(str::to_owned), (*u).to_owned())).collect())
    }

    fn written(scope: &Scope, namespace: Option<&str>, local: &str, attribute: bool) -> (String, Option<(Option<String>, String)>) {
        let w = scope.write(namespace, local, attribute);
        (w.name, w.declare)
    }

    #[test]
    fn a_name_is_written_with_what_is_in_scope() {
        let s = scope(&[(None, "urn:a"), (Some("b"), "urn:b")]);
        assert_eq!(written(&s, Some("urn:a"), "x", false), ("x".to_owned(), None), "the default namespace");
        assert_eq!(written(&s, Some("urn:b"), "x", false), ("b:x".to_owned(), None), "a prefix");
        assert_eq!(written(&s, Some("urn:b"), "x", true), ("b:x".to_owned(), None), "an attribute takes the prefix too");
        // An attribute is not in the default namespace: it needs a prefix.
        assert_eq!(written(&s, Some("urn:a"), "x", true), ("ns1:x".to_owned(), Some((Some("ns1".to_owned()), "urn:a".to_owned()))));
        // No namespace under a default one is `xmlns=""`; an attribute needs nothing.
        assert_eq!(written(&s, None, "x", false), ("x".to_owned(), Some((None, String::new()))));
        assert_eq!(written(&s, None, "x", true), ("x".to_owned(), None));
        // A namespace that is not in scope: a prefix, since the default is taken.
        assert_eq!(written(&s, Some("urn:c"), "x", false), ("ns1:x".to_owned(), Some((Some("ns1".to_owned()), "urn:c".to_owned()))));
    }

    #[test]
    fn a_free_default_namespace_is_used_for_an_element_and_a_new_prefix_is_new() {
        let free = scope(&[]);
        assert_eq!(written(&free, Some("urn:c"), "x", false), ("x".to_owned(), Some((None, "urn:c".to_owned()))));
        assert_eq!(written(&free, None, "x", false), ("x".to_owned(), None));
        assert_eq!(written(&free, Some("urn:c"), "x", true), ("ns1:x".to_owned(), Some((Some("ns1".to_owned()), "urn:c".to_owned()))));
        // `ns1` is in use for something else: the next one.
        let busy = scope(&[(Some("ns1"), "urn:x"), (None, "urn:a")]);
        assert_eq!(written(&busy, Some("urn:c"), "x", false).0, "ns2:x");
        // The default namespace undeclared again is free.
        let undeclared = scope(&[(None, "urn:a"), (None, "")]);
        assert_eq!(written(&undeclared, None, "x", false), ("x".to_owned(), None));
        assert_eq!(undeclared.resolve(None), None);
    }

    #[test]
    fn a_prefix_that_was_shadowed_does_not_stand_for_its_first_namespace() {
        // `p` meant `urn:a` and now means `urn:b`: `urn:a` is not reachable through it.
        let s = scope(&[(Some("p"), "urn:a"), (Some("p"), "urn:b")]);
        assert_eq!(written(&s, Some("urn:b"), "x", false).0, "p:x");
        assert_eq!(written(&s, Some("urn:a"), "x", false), ("x".to_owned(), Some((None, "urn:a".to_owned()))), "the default namespace is free");
        assert_eq!(written(&s, Some("urn:a"), "x", true).0, "ns1:x", "an attribute needs a prefix");
        assert_eq!(s.expand("p:x", true), QName { namespace: Some("urn:b".to_owned()), local: "x".to_owned() });
    }

    #[test]
    fn names_are_expanded_as_the_namespaces_say() {
        let s = scope(&[(None, "urn:a"), (Some("b"), "urn:b")]);
        assert_eq!(s.expand("x", true).namespace.as_deref(), Some("urn:a"));
        assert_eq!(s.expand("x", false).namespace, None, "an unprefixed attribute");
        assert_eq!(s.expand("b:x", false).namespace.as_deref(), Some("urn:b"));
        // A prefix nobody declared leaves the name as written.
        assert_eq!(s.expand("q:x", true), QName { namespace: None, local: "q:x".to_owned() });
        assert_eq!(s.expand("xml:lang", false).namespace.as_deref(), Some(XML_NAMESPACE));
        assert_eq!(written(&s, Some(XML_NAMESPACE), "lang", true), ("xml:lang".to_owned(), None));
    }

    #[test]
    fn a_declaration_is_written_with_its_value_escaped() {
        assert_eq!(declaration_text(&(Some("p".to_owned()), "urn:a&b".to_owned())), " xmlns:p=\"urn:a&amp;b\"");
        assert_eq!(declaration_text(&(None, String::new())), " xmlns=\"\"");
    }
}
