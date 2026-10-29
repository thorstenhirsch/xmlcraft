//! Wildcards: the namespaces they allow, and the intersection and union of two of them (the rules for combining the attribute wildcards
//! of attribute groups and of a type with its base).

use super::Builder;
use crate::model;
use crate::schema::{NamespaceSet, Wildcard};

impl Builder<'_> {
    /// The wildcard `w` of a definition in document `doc`.
    pub(super) fn wildcard(&self, doc: usize, w: &model::Wildcard) -> Wildcard {
        let target = self.namespace_of(doc);
        Wildcard { constraint: NamespaceSet::from_constraint(&w.namespace, target.as_deref()), process_contents: w.process_contents }
    }
}

/// The namespaces both sets allow, or `None` when that is not a set the language can write (the namespaces other than two different
/// ones).
pub(super) fn intersect(a: &NamespaceSet, b: &NamespaceSet) -> Option<NamespaceSet> {
    use NamespaceSet::*;
    Some(match (a, b) {
        (Any, other) | (other, Any) => other.clone(),
        (Not(x), Not(y)) => match (x, y) {
            (x, y) if x == y => Not(x.clone()),
            // Not having no namespace is only weaker than not having a namespace and the other.
            (None, other) | (other, None) => Not(other.clone()),
            _ => return None,
        },
        (Not(_), List(list)) => List(list.iter().filter(|n| a.allows(n.as_deref())).cloned().collect()),
        (List(list), Not(_)) => List(list.iter().filter(|n| b.allows(n.as_deref())).cloned().collect()),
        (List(x), List(y)) => List(x.iter().filter(|n| y.contains(n)).cloned().collect()),
    })
}

/// The namespaces either set allows, or `None` when that is not a set the language can write (all but one namespace, and no
/// namespace).
pub(super) fn union(a: &NamespaceSet, b: &NamespaceSet) -> Option<NamespaceSet> {
    use NamespaceSet::*;
    Some(match (a, b) {
        (Any, _) | (_, Any) => Any,
        (Not(x), Not(y)) => {
            if x == y {
                Not(x.clone())
            } else {
                // Each allows what the other does not, except no namespace, which neither allows.
                Not(None)
            }
        }
        (Not(negated), List(list)) | (List(list), Not(negated)) => {
            let has_absent = list.contains(&None);
            match negated {
                Some(_) => match (list.contains(negated), has_absent) {
                    (true, true) => Any,
                    (true, false) => Not(None),
                    // Everything but the negated namespace, and no namespace too: there is no way to say that.
                    (false, true) => return None,
                    (false, false) => Not(negated.clone()),
                },
                None => {
                    if has_absent {
                        Any
                    } else {
                        Not(None)
                    }
                }
            }
        }
        (List(x), List(y)) => {
            let mut all = x.clone();
            for n in y {
                if !all.contains(n) {
                    all.push(n.clone());
                }
            }
            List(all)
        }
    })
}

/// Combine two wildcards by intersection; the processing is the first one's.
pub(super) fn intersect_wildcards(first: &Wildcard, second: &Wildcard) -> Option<Wildcard> {
    Some(Wildcard { constraint: intersect(&first.constraint, &second.constraint)?, process_contents: first.process_contents })
}

/// Combine two wildcards by union; the processing is the first one's.
pub(super) fn union_wildcards(first: &Wildcard, second: &Wildcard) -> Option<Wildcard> {
    Some(Wildcard { constraint: union(&first.constraint, &second.constraint)?, process_contents: first.process_contents })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sets() -> Vec<NamespaceSet> {
        let n = |s: &str| Some(s.to_owned());
        let mut sets = vec![NamespaceSet::Any, NamespaceSet::Not(None), NamespaceSet::Not(n("a")), NamespaceSet::Not(n("b"))];
        for list in [
            vec![],
            vec![None],
            vec![n("a")],
            vec![n("b")],
            vec![n("a"), n("b")],
            vec![None, n("a")],
            vec![None, n("c")],
            vec![n("a"), n("c")],
            vec![None, n("a"), n("b")],
        ] {
            sets.push(NamespaceSet::List(list));
        }
        sets
    }

    // The namespaces to ask about: the ones the sets mention, none, and one nothing mentions.
    fn universe() -> Vec<Option<&'static str>> {
        vec![None, Some("a"), Some("b"), Some("c"), Some("z")]
    }

    #[test]
    fn the_intersection_allows_what_both_allow() {
        for a in sets() {
            for b in sets() {
                match intersect(&a, &b) {
                    Some(both) => {
                        for n in universe() {
                            assert_eq!(both.allows(n), a.allows(n) && b.allows(n), "{a:?} and {b:?}, {n:?}");
                        }
                    }
                    None => assert!(
                        matches!((&a, &b), (NamespaceSet::Not(Some(x)), NamespaceSet::Not(Some(y))) if x != y),
                        "{a:?} and {b:?} can be written"
                    ),
                }
            }
        }
    }

    #[test]
    fn the_union_allows_what_either_allows() {
        for a in sets() {
            for b in sets() {
                match union(&a, &b) {
                    Some(either) => {
                        for n in universe() {
                            assert_eq!(either.allows(n), a.allows(n) || b.allows(n), "{a:?} and {b:?}, {n:?}");
                        }
                    }
                    None => {
                        // The only union that cannot be written: all but one namespace together with no namespace.
                        let unwritable = |negated: &NamespaceSet, list: &NamespaceSet| matches!((negated, list), (NamespaceSet::Not(Some(x)), NamespaceSet::List(l)) if l.contains(&None) && !l.contains(&Some(x.clone())));
                        assert!(unwritable(&a, &b) || unwritable(&b, &a), "{a:?} and {b:?} can be written");
                    }
                }
            }
        }
    }
}
