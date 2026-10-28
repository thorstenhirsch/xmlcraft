//! The values of the attributes of schema elements: the little languages `minOccurs`, `block`, `namespace` and the qualified names.

use xmlcraft_datatypes::{collapse, is_any_uri, is_ncname};

use crate::dom::Dom;
use crate::model::{DerivationSet, Form, MaxOccurs, NamespaceConstraint, NamespaceToken, ProcessContents, QName};

/// A non-negative integer that fits `u32`. (A larger one is taken as the largest, which no schema needs.)
pub fn non_negative(value: &str) -> Result<u32, String> {
    let v = collapse(value);
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("'{value}' is not a non-negative integer"));
    }
    Ok(v.parse().unwrap_or(u32::MAX))
}

pub fn max_occurs(value: &str) -> Result<MaxOccurs, String> {
    let v = collapse(value);
    if v == "unbounded" {
        return Ok(MaxOccurs::Unbounded);
    }
    non_negative(&v).map(MaxOccurs::Bounded).map_err(|_| format!("'{value}' is not 'unbounded' or a non-negative integer"))
}

pub fn boolean(value: &str) -> Result<bool, String> {
    match collapse(value).as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(format!("'{value}' is not true or false")),
    }
}

pub fn form(value: &str) -> Result<Form, String> {
    match collapse(value).as_str() {
        "qualified" => Ok(Form::Qualified),
        "unqualified" => Ok(Form::Unqualified),
        _ => Err(format!("'{value}' is not 'qualified' or 'unqualified'")),
    }
}

pub fn process_contents(value: &str) -> Result<ProcessContents, String> {
    match collapse(value).as_str() {
        "strict" => Ok(ProcessContents::Strict),
        "lax" => Ok(ProcessContents::Lax),
        "skip" => Ok(ProcessContents::Skip),
        _ => Err(format!("'{value}' is not 'strict', 'lax' or 'skip'")),
    }
}

/// `#all`, or a list of the `allowed` words.
pub fn derivation_set(value: &str, allowed: &[&str]) -> Result<DerivationSet, String> {
    let v = collapse(value);
    if v == "#all" {
        return Ok(DerivationSet {
            extension: allowed.contains(&"extension"),
            restriction: allowed.contains(&"restriction"),
            substitution: allowed.contains(&"substitution"),
            list: allowed.contains(&"list"),
            union: allowed.contains(&"union"),
        });
    }
    let mut set = DerivationSet::default();
    for word in v.split(' ').filter(|w| !w.is_empty()) {
        if !allowed.contains(&word) {
            return Err(format!("'{word}' is not allowed here; expected #all or a list of {}", allowed.join(", ")));
        }
        match word {
            "extension" => set.extension = true,
            "restriction" => set.restriction = true,
            "substitution" => set.substitution = true,
            "list" => set.list = true,
            "union" => set.union = true,
            _ => {}
        }
    }
    Ok(set)
}

/// The `namespace` attribute of a wildcard: `##any`, `##other`, or a list of namespace names, `##local` and `##targetNamespace`.
pub fn namespace_constraint(value: &str) -> Result<NamespaceConstraint, String> {
    let v = collapse(value);
    match v.as_str() {
        "##any" => return Ok(NamespaceConstraint::Any),
        "##other" => return Ok(NamespaceConstraint::Other),
        _ => {}
    }
    let mut tokens = Vec::new();
    for word in v.split(' ').filter(|w| !w.is_empty()) {
        tokens.push(match word {
            "##local" => NamespaceToken::Local,
            "##targetNamespace" => NamespaceToken::TargetNamespace,
            "##any" | "##other" => return Err(format!("'{word}' cannot be in a list")),
            _ if word.starts_with("##") => return Err(format!("'{word}' is not a namespace token")),
            _ if is_any_uri(word) => NamespaceToken::Uri(word.to_owned()),
            _ => return Err(format!("'{word}' is not a namespace name")),
        });
    }
    if tokens.is_empty() {
        return Err("it is empty".to_owned());
    }
    Ok(NamespaceConstraint::List(tokens))
}

/// A qualified name written in an attribute of the element `at`: the prefix is looked up in the declarations in scope there. An
/// unprefixed name is in the default namespace, if there is one. The error is the reason.
pub fn qname(dom: &Dom, at: usize, value: &str) -> Result<QName, String> {
    let v = collapse(value);
    let (prefix, local) = match v.split_once(':') {
        Some((p, l)) => (Some(p), l),
        None => (None, v.as_str()),
    };
    if !is_ncname(local) || prefix.is_some_and(|p| !is_ncname(p)) {
        return Err(format!("'{value}' is not a qualified name"));
    }
    match prefix {
        Some(p) => dom
            .lookup(at, Some(p))
            .map(|ns| QName { namespace: Some(ns), local: local.to_owned() })
            .ok_or_else(|| format!("the prefix '{p}' is not declared")),
        None => Ok(QName { namespace: dom.lookup(at, None), local: local.to_owned() }),
    }
}

/// A list of qualified names (`memberTypes`).
pub fn qname_list(dom: &Dom, at: usize, value: &str) -> Result<Vec<QName>, String> {
    collapse(value).split(' ').filter(|w| !w.is_empty()).map(|w| qname(dom, at, w)).collect()
}
