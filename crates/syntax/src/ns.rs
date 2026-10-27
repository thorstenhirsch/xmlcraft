//! XML Namespaces 1.0: prefix scopes and qualified names.

use std::collections::HashMap;

/// The namespace name the prefix `xml` is bound to, always.
pub const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";
/// The namespace name of `xmlns` declarations; it can never be bound to a prefix.
pub const XMLNS_NAMESPACE: &str = "http://www.w3.org/2000/xmlns/";

/// The prefix bindings in force at a point of a document. Scopes nest: each element opens one, and its closing drops the bindings the
/// element declared.
#[derive(Clone, Debug, Default)]
pub struct NsScope {
    /// For each prefix (`None` is the default namespace), its bindings from outermost to innermost.
    bindings: HashMap<Option<String>, Vec<String>>,
    /// For each open scope, the prefixes it declared.
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
            if let Some(stack) = self.bindings.get_mut(&prefix) {
                stack.pop();
                if stack.is_empty() {
                    self.bindings.remove(&prefix);
                }
            }
        }
    }

    /// Bind `prefix` (`None` for the default namespace) to `uri` in the innermost scope. An empty `uri` for the default namespace
    /// undeclares it. Does nothing when no scope is open.
    pub fn declare(&mut self, prefix: Option<&str>, uri: &str) {
        let Some(frame) = self.frames.last_mut() else { return };
        let key = prefix.map(str::to_owned);
        frame.push(key.clone());
        self.bindings.entry(key).or_default().push(uri.to_owned());
    }

    /// The namespace name `prefix` stands for here. `xml` is always bound; the default namespace is `None` when undeclared or
    /// undeclared again with `xmlns=""`.
    pub fn lookup(&self, prefix: Option<&str>) -> Option<&str> {
        if prefix == Some("xml") {
            return Some(XML_NAMESPACE);
        }
        let uri = self.bindings.get(&prefix.map(str::to_owned))?.last()?;
        (prefix.is_some() || !uri.is_empty()).then_some(uri.as_str())
    }
}

/// Split a qualified name into its prefix and local part. `None` if it is not a QName: more than one colon, or a colon at either end.
pub fn split_qname(name: &str) -> Option<(Option<&str>, &str)> {
    match name.split_once(':') {
        None => Some((None, name)),
        Some((prefix, local)) if !prefix.is_empty() && !local.is_empty() && !local.contains(':') => Some((Some(prefix), local)),
        Some(_) => None,
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
        for bad in [":a", "a:", "a:b:c", ":", "::"] {
            assert_eq!(split_qname(bad), None, "{bad}");
        }
    }
}
