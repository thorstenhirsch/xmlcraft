//! XML Namespaces 1.0: prefix scopes and qualified names.

use std::collections::HashMap;

use crate::chars::{is_name_char, is_name_start_char};

/// The namespace name the prefix `xml` is bound to, always.
pub const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
/// The namespace name of `xmlns` declarations; it can never be bound to a prefix.
pub const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// The prefix bindings in force at a point of a document. Scopes nest: each element opens one, and its closing drops the bindings the
/// element declared.
#[derive(Clone, Debug, Default)]
pub struct NsScope {
    /// For each prefix, its bindings from outermost to innermost.
    prefixed: HashMap<String, Vec<String>>,
    /// The default namespace's bindings from outermost to innermost; an empty one is `xmlns=""`.
    default: Vec<String>,
    /// For each open scope, the prefixes it declared (`None` is the default namespace).
    frames: Vec<Vec<Option<String>>>,
}

impl NsScope {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many scopes are open.
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    pub fn push_scope(&mut self) {
        self.frames.push(Vec::new());
    }

    /// Close the innermost scope, dropping what it declared. Does nothing when no scope is open.
    pub fn pop_scope(&mut self) {
        let Some(declared) = self.frames.pop() else { return };
        for prefix in declared {
            match prefix {
                None => {
                    self.default.pop();
                }
                Some(prefix) => {
                    if let Some(stack) = self.prefixed.get_mut(&prefix) {
                        stack.pop();
                        if stack.is_empty() {
                            self.prefixed.remove(&prefix);
                        }
                    }
                }
            }
        }
    }

    /// Bind `prefix` (`None` for the default namespace) to `uri` in the innermost scope. An empty `uri` for the default namespace
    /// undeclares it. Returns whether the binding was made: it is not when no scope is open, when `prefix` is not an NCName or is
    /// `xml` or `xmlns`, when a prefix is bound to the empty name, or when `uri` is the namespace name of `xml` or `xmlns`.
    pub fn declare(&mut self, prefix: Option<&str>, uri: &str) -> bool {
        let reserved = uri == XML_NAMESPACE || uri == XMLNS_NAMESPACE;
        let allowed = match prefix {
            None => !reserved,
            Some(p) => is_ncname(p) && !matches!(p, "xml" | "xmlns") && !uri.is_empty() && !reserved,
        };
        if !allowed {
            return false;
        }
        let Some(frame) = self.frames.last_mut() else { return false };
        frame.push(prefix.map(str::to_owned));
        match prefix {
            None => self.default.push(uri.to_owned()),
            Some(p) => self.prefixed.entry(p.to_owned()).or_default().push(uri.to_owned()),
        }
        true
    }

    /// The namespace name `prefix` stands for here. `xml` is always bound; the default namespace is `None` when undeclared or
    /// undeclared again with `xmlns=""`.
    pub fn lookup(&self, prefix: Option<&str>) -> Option<&str> {
        let uri = match prefix {
            Some("xml") => return Some(XML_NAMESPACE),
            Some(p) => self.prefixed.get(p)?.last()?,
            None => self.default.last()?,
        };
        (!uri.is_empty()).then_some(uri.as_str())
    }
}

/// Whether `name` is an `NCName`: a `Name` without colons.
pub fn is_ncname(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c != ':' && is_name_start_char(c)) && chars.all(|c| c != ':' && is_name_char(c))
}

/// Split a qualified name into its prefix and local part. `None` if it is not a QName: not a `Name`, or more than one colon, or a
/// prefix or local part that is not an `NCName` (so a colon at either end, or a part starting with a digit).
pub fn split_qname(name: &str) -> Option<(Option<&str>, &str)> {
    match name.split_once(':') {
        None => is_ncname(name).then_some((None, name)),
        Some((prefix, local)) => (is_ncname(prefix) && is_ncname(local)).then_some((Some(prefix), local)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_nest_and_unwind() {
        let mut ns = NsScope::new();
        ns.push_scope();
        ns.declare(Some("a"), "urn:outer");
        ns.declare(None, "urn:default");
        ns.push_scope();
        ns.declare(Some("a"), "urn:inner");
        assert_eq!(ns.lookup(Some("a")), Some("urn:inner"));
        assert_eq!(ns.lookup(None), Some("urn:default"));
        ns.push_scope();
        ns.declare(None, "");
        assert_eq!(ns.lookup(None), None, "xmlns=\"\" undeclares the default namespace");
        ns.pop_scope();
        ns.pop_scope();
        assert_eq!(ns.lookup(Some("a")), Some("urn:outer"));
        assert_eq!(ns.lookup(None), Some("urn:default"));
        ns.pop_scope();
        assert_eq!((ns.lookup(Some("a")), ns.lookup(None), ns.depth()), (None, None, 0));
    }

    #[test]
    fn xml_is_always_bound_and_misuse_is_harmless() {
        let mut ns = NsScope::new();
        assert_eq!(ns.lookup(Some("xml")), Some(XML_NAMESPACE));
        ns.pop_scope(); // nothing open
        ns.declare(Some("p"), "urn:x"); // nothing open
        assert_eq!(ns.lookup(Some("p")), None);
        assert_eq!(ns.lookup(Some("xmlns")), None);
    }

    #[test]
    fn qnames() {
        assert_eq!(split_qname("a"), Some((None, "a")));
        assert_eq!(split_qname("p:a"), Some((Some("p"), "a")));
        assert_eq!(split_qname("é:日"), Some((Some("é"), "日")));
        // Colon placement, and parts that are not NCNames: empty, or starting with a digit, a hyphen or a dot.
        for bad in ["", ":a", "a:", "a:b:c", ":", "::", "1:a", "p:1", "1", "-a", "p:-a", ".a", "p:.a", "a b", "p:a b"] {
            assert_eq!(split_qname(bad), None, "{bad:?}");
        }
        assert!(is_ncname("a-b.c1") && !is_ncname("a:b") && !is_ncname("") && !is_ncname("1a"));
    }

    #[test]
    fn declare_refuses_what_namespaces_forbid() {
        let mut ns = NsScope::new();
        ns.push_scope();
        assert!(ns.declare(Some("p"), "urn:p"));
        assert!(ns.declare(None, "urn:d"));
        assert!(ns.declare(None, ""), "xmlns=\"\" is how the default namespace is undeclared");
        assert_eq!(ns.lookup(None), None);
        // A prefix bound to nothing, the reserved prefixes, the reserved names and prefixes that are not NCNames.
        for (prefix, uri) in [
            (Some("q"), ""),
            (Some("xml"), "urn:x"),
            (Some("xml"), XML_NAMESPACE),
            (Some("xmlns"), "urn:x"),
            (Some("q"), XML_NAMESPACE),
            (Some("q"), XMLNS_NAMESPACE),
            (None, XML_NAMESPACE),
            (None, XMLNS_NAMESPACE),
            (Some(""), "urn:x"),
            (Some("a:b"), "urn:x"),
            (Some("1a"), "urn:x"),
        ] {
            assert!(!ns.declare(prefix, uri), "{prefix:?} {uri:?}");
        }
        assert_eq!(ns.lookup(Some("q")), None);
        assert_eq!(ns.lookup(Some("p")), Some("urn:p"));
        assert_eq!(ns.lookup(Some("xml")), Some(XML_NAMESPACE));
        assert_eq!(ns.lookup(Some("xmlns")), None);
        ns.pop_scope();
        assert!(!ns.declare(Some("p"), "urn:p"), "no scope is open");
    }
}
