//! Content models: particles, with the references to model groups expanded.

use std::rc::Rc;

use super::*;

impl<'a> Builder<'a> {
    /// Use up `n` of the particles a schema may have. The schema is refused (once) when they are gone.
    fn spend(&mut self, doc: usize, span: &Range<usize>, n: usize) -> bool {
        match self.particles_left.checked_sub(n) {
            Some(left) => {
                self.particles_left = left;
                true
            }
            None => {
                if !self.particle_limit_reported {
                    self.particle_limit_reported = true;
                    self.report(doc, Code::XsdLimit, span.clone(), &["the content models have too many particles, group references counted"]);
                }
                self.particles_left = 0;
                false
            }
        }
    }

    /// The particle `p` written in document `doc`; the elements declared in it are local to the type `scope` (none: to a model group).
    pub(super) fn particle(&mut self, doc: usize, p: &'a Particle, scope: Option<TypeId>) -> Option<SParticle> {
        if !self.spend(doc, &p.span, 1) {
            return None;
        }
        let max = match p.max_occurs {
            MaxOccurs::Bounded(n) => Some(n),
            MaxOccurs::Unbounded => None,
        };
        let term = match &p.term {
            Term::Element(decl) => match &decl.reference {
                Some(reference) => STerm::Element(self.global_element(doc, reference)?),
                None => STerm::Element(self.element_decl(doc, decl, scope, None)?),
            },
            Term::GroupRef(reference) => self.group(doc, reference)?.term.clone(),
            Term::Sequence(children) => STerm::Sequence(self.particles(doc, children, scope)),
            Term::Choice(children) => STerm::Choice(self.particles(doc, children, scope)),
            Term::All(children) => {
                let built = self.particles(doc, children, scope);
                if built.iter().any(|c| c.max.is_none_or(|m| m > 1)) {
                    self.report(doc, Code::XsdComponent, p.span.clone(), &["xs:all", "an element in it may occur at most once"]);
                }
                STerm::All(built)
            }
            Term::Any(w) => STerm::Wildcard(self.wildcard(doc, w)),
        };
        Some(SParticle { min: p.min_occurs, max, term })
    }

    fn particles(&mut self, doc: usize, list: &'a [Particle], scope: Option<TypeId>) -> Vec<SParticle> {
        list.iter().filter_map(|p| self.particle(doc, p, scope)).collect()
    }

    /// The model group a reference names, with its occurrence of one; it is built once.
    fn group(&mut self, doc: usize, reference: &QRef) -> Option<Rc<SParticle>> {
        let name = self.visible_name(doc, reference, "group")?;
        if let Some(done) = self.group_memo.get(&name).cloned() {
            let (group, nodes) = done?;
            return self.spend(doc, &reference.span, nodes).then_some(group);
        }
        let key = (Kind::Group, name.clone());
        if self.progress.contains(&key) {
            if self.reported.insert(key) {
                self.report(doc, Code::XsdCircular, reference.span.clone(), &[&name.to_string(), "the group contains itself"]);
            }
            return None;
        }
        let Some(&place) = self.group_places.get(&name) else {
            self.report(doc, Code::XsdUnresolved, reference.span.clone(), &["group", &name.to_string()]);
            return None;
        };
        let Some(Item::Group(def)) = self.item(place) else { return None };
        if self.depth >= MAX_DEPTH {
            self.report(doc, Code::XsdTooDeep, reference.span.clone(), &[&name.to_string()]);
            return None;
        }
        self.progress.insert(key.clone());
        self.depth += 1;
        let before = self.particles_left;
        let built = def.content.as_ref().and_then(|content| self.particle(place.0, content, None));
        let nodes = before.saturating_sub(self.particles_left).max(1);
        self.depth -= 1;
        self.progress.remove(&key);
        let built = built.map(|p| (Rc::new(p), nodes));
        self.group_memo.insert(name, built.clone());
        built.map(|(group, _)| group)
    }

    /// Can the particle match nothing?
    pub(super) fn emptiable(p: &SParticle) -> bool {
        if p.min == 0 {
            return true;
        }
        match &p.term {
            STerm::Element(_) | STerm::Wildcard(_) => false,
            STerm::Sequence(children) | STerm::All(children) => children.iter().all(Self::emptiable),
            STerm::Choice(children) => children.is_empty() || children.iter().any(Self::emptiable),
        }
    }

    /// A particle that can match nothing but the empty content, so that a type with only this has no elements.
    pub(super) fn vacuous(p: &SParticle) -> bool {
        p.max == Some(0)
            || match &p.term {
                STerm::Sequence(children) | STerm::All(children) => children.iter().all(Self::vacuous),
                // A choice with nothing to choose from cannot be matched at all, unless it may occur no times.
                STerm::Choice(children) if children.is_empty() => p.min == 0,
                STerm::Choice(children) => children.iter().all(Self::vacuous),
                STerm::Element(_) | STerm::Wildcard(_) => false,
            }
    }
}
