//! Names, and the terms a content model is made of: an element, or a wildcard that lets in the elements of some namespaces.

/// A qualified name: the namespace name (`None` for no namespace) and the local name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name {
    pub namespace: Option<String>,
    pub local: String,
}

impl Name {
    pub fn new(namespace: Option<&str>, local: &str) -> Self {
        Self { namespace: namespace.map(str::to_owned), local: local.to_owned() }
    }

    /// A name in no namespace (all a DTD has).
    pub fn local(local: &str) -> Self {
        Self::new(None, local)
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.namespace {
            Some(ns) => write!(f, "{{{ns}}}{}", self.local),
            None => f.write_str(&self.local),
        }
    }
}

/// Which namespaces a wildcard lets in (XML Schema 1.0, "Wildcard Schema Component").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamespaceConstraint {
    /// `##any`: every namespace, and no namespace.
    Any,
    /// `##other`: every namespace except the target namespace of the schema (`None` when it has none) and no namespace.
    Other(Option<String>),
    /// A list of namespaces; `None` in it is no namespace (`##local`).
    Set(Vec<Option<String>>),
}

impl NamespaceConstraint {
    /// Whether an element in `namespace` is let in.
    pub fn allows(&self, namespace: Option<&str>) -> bool {
        match self {
            NamespaceConstraint::Any => true,
            NamespaceConstraint::Other(target) => namespace.is_some() && namespace != target.as_deref(),
            NamespaceConstraint::Set(list) => list.iter().any(|n| n.as_deref() == namespace),
        }
    }

    /// Whether some namespace is let in by both.
    pub fn overlaps(&self, other: &NamespaceConstraint) -> bool {
        use NamespaceConstraint::{Any, Other, Set};
        match (self, other) {
            (Any, Set(list)) | (Set(list), Any) => !list.is_empty(),
            (Any, _) | (_, Any) => true,
            // There are always namespaces besides the two named.
            (Other(_), Other(_)) => true,
            (Other(target), Set(list)) | (Set(list), Other(target)) => list.iter().any(|n| n.is_some() && n != target),
            (Set(a), Set(b)) => a.iter().any(|n| b.contains(n)),
        }
    }
}

/// What a particle of a content model stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// The element with this name.
    Element(Name),
    /// Any element of the namespaces the constraint lets in.
    Wildcard(NamespaceConstraint),
}

impl Term {
    /// Whether an element called `name` is this term.
    pub fn matches(&self, name: &Name) -> bool {
        match self {
            Term::Element(n) => n == name,
            Term::Wildcard(constraint) => constraint.allows(name.namespace.as_deref()),
        }
    }

    /// Whether some element name is matched by both terms: two particles that can compete for the same element
    /// ("Unique Particle Attribution").
    pub fn overlaps(&self, other: &Term) -> bool {
        match (self, other) {
            (Term::Element(a), Term::Element(b)) => a == b,
            (Term::Element(n), Term::Wildcard(c)) | (Term::Wildcard(c), Term::Element(n)) => c.allows(n.namespace.as_deref()),
            (Term::Wildcard(a), Term::Wildcard(b)) => a.overlaps(b),
        }
    }
}

impl std::fmt::Display for Term {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Term::Element(name) => name.fmt(f),
            Term::Wildcard(NamespaceConstraint::Any) => f.write_str("##any"),
            Term::Wildcard(NamespaceConstraint::Other(_)) => f.write_str("##other"),
            Term::Wildcard(NamespaceConstraint::Set(list)) => {
                f.write_str("{")?;
                for (i, ns) in list.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    f.write_str(ns.as_deref().unwrap_or("##local"))?;
                }
                f.write_str("}:*")
            }
        }
    }
}
